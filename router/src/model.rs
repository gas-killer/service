//! Model descriptor for the shard coordinator.
//!
//! The DAG planner in `shard.rs` used to hardcode the 0.6B model's structural
//! facts (all layers are dense attention, KV-only boundary state, a 3-word
//! `packedConfig`, the `Qwen3SegEngine` call ABI). Weight-sharding a much larger,
//! architecturally heterogeneous model — Qwen3.5-35B-A3B: 40 layers in a
//! 30-DeltaNet / 10-full-attention hybrid, MoE every layer, a 4-word
//! `packedConfig`, and a `Qwen35SegEngine` call ABI whose boundary state is
//! layer-type dependent — means those facts can no longer be implicit.
//!
//! [`ModelSpec`] captures them so the coordinator is parameterized on the model
//! family (selected by `GK_SHARD_MODEL`, default `qwen3-0.6b`) rather than
//! assuming one. It also owns the two things that genuinely differ per family:
//! the per-segment **call encoding** ([`ModelSpec::encode_forward`] /
//! [`encode_argmax`](ModelSpec::encode_argmax)) and the boundary **state wire
//! format** used to thread blobs between segments (via [`ModelSpec::layer_kind`]
//! and the byte-size helpers).

use anyhow::{Context, Result, bail};
use alloy::sol_types::SolCall;
use alloy_primitives::{B256, U256, keccak256};

use gas_killer_common::shard::{SegCall, SegSpan, argmaxRangeCall, forwardRangeCall, seg35};

/// Which token mixer a decoder layer uses. Relevant to the boundary state wire
/// format (a full-attention layer threads a KV cache that GROWS per position; a
/// DeltaNet layer threads a fixed-size recurrent snapshot that is REPLACED each
/// position) and to the cheap-cut analysis in [`ModelSpec::stage_bounds`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    /// Standard KV-cache attention (every 0.6B layer; every 4th 35B layer).
    Attention,
    /// Linear-attention DeltaNet recurrence (35B non-full layers). Threads a
    /// ~2.1 MB S snapshot + conv state per layer — the expensive boundary.
    DeltaNet,
}

/// Model family selected by `GK_SHARD_MODEL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelId {
    /// `qwen3-0.6b` — the proven Stage-A path (`Qwen3SegEngine`).
    Qwen3_06B,
    /// `qwen35` — Qwen3.5-35B-A3B (`Qwen35SegEngine`).
    Qwen35,
}

/// The structural facts the DAG planner needs about a model family, plus the
/// per-family call encoding / boundary-state layout.
#[derive(Debug, Clone)]
pub struct ModelSpec {
    pub id: ModelId,
    /// Human-readable id (the `GK_SHARD_MODEL` value).
    pub name: &'static str,
    /// Decoder layer count (structural default; the per-run `InferRequest` still
    /// carries the deployed model's `n_layers` and remains authoritative for the
    /// DAG arithmetic, so a synthetic fixture with a different depth still runs).
    pub n_layers: u64,
    /// `Some(interval)` => layer `l` is full-attention iff `(l + 1) % interval == 0`
    /// (35B: `interval = 4` → layers 3,7,…,39 full, the rest DeltaNet).
    /// `None` => every layer is dense attention (0.6B).
    pub full_attention_interval: Option<u64>,
    /// Hidden size.
    pub dim: u64,
    /// Full-attention KV dim per position per side (`nKv * headDim`).
    pub kvd: u64,
    /// Classifier / embedding rows.
    pub vocab: u64,
    /// `packedConfig` word count (0.6B: 3, 35B: 4). Pins which call ABI applies.
    pub packed_config_words: usize,
    // ---- DeltaNet boundary-snapshot dims (35B only; 0 for a pure-attention model).
    /// DeltaNet causal-conv channel count (`2*keyDim + valueDim`).
    pub conv_dim: u64,
    /// DeltaNet causal-conv kernel width.
    pub conv_k: u64,
    /// DeltaNet value heads.
    pub n_v_heads: u64,
    /// DeltaNet key head dim.
    pub d_k: u64,
    /// DeltaNet value head dim.
    pub d_v: u64,
}

impl ModelSpec {
    /// The existing 0.6B path — every layer dense attention, KV-only boundary
    /// state, 3-word packed config, `Qwen3SegEngine` ABI.
    pub fn qwen3_06b() -> Self {
        Self {
            id: ModelId::Qwen3_06B,
            name: "qwen3-0.6b",
            n_layers: 28,
            full_attention_interval: None,
            dim: 1024,
            kvd: 1024,
            vocab: 151936,
            packed_config_words: 3,
            conv_dim: 0,
            conv_k: 0,
            n_v_heads: 0,
            d_k: 0,
            d_v: 0,
        }
    }

    /// Qwen3.5-35B-A3B — hybrid DeltaNet/attention stack, MoE every layer,
    /// 4-word packed config, `Qwen35SegEngine` ABI. Constants from
    /// `.context/qwen35/SPEC.md §0` (and confirmed by the engine agent):
    /// L=40, fullInterval=4, dim=2048, kvd = nKv*headDim = 2*256 = 512,
    /// vocab=248320; DeltaNet: convDim=8192, convK=4, nVH=32, dK=dV=128.
    pub fn qwen35() -> Self {
        Self {
            id: ModelId::Qwen35,
            name: "qwen35",
            n_layers: 40,
            full_attention_interval: Some(4),
            dim: 2048,
            kvd: 512,
            vocab: 248320,
            packed_config_words: 4,
            conv_dim: 8192,
            conv_k: 4,
            n_v_heads: 32,
            d_k: 128,
            d_v: 128,
        }
    }

    /// Select the spec from `GK_SHARD_MODEL` (default `qwen3-0.6b`).
    pub fn from_env() -> Result<Self> {
        match std::env::var("GK_SHARD_MODEL")
            .unwrap_or_else(|_| "qwen3-0.6b".to_string())
            .as_str()
        {
            "qwen3-0.6b" | "qwen3-0.6B" | "0.6b" => Ok(Self::qwen3_06b()),
            "qwen35" | "qwen3.5" | "qwen3.5-35b" => Ok(Self::qwen35()),
            other => bail!(
                "GK_SHARD_MODEL={other:?} is not a known model (expected \"qwen3-0.6b\" or \"qwen35\")"
            ),
        }
    }

    /// Whether layer `l` is full-attention (vs DeltaNet).
    pub fn is_full_attention(&self, l: u64) -> bool {
        match self.full_attention_interval {
            None => true,
            Some(interval) => interval != 0 && (l + 1).is_multiple_of(interval),
        }
    }

    /// The token mixer at layer `l`.
    pub fn layer_kind(&self, l: u64) -> LayerKind {
        if self.is_full_attention(l) {
            LayerKind::Attention
        } else {
            LayerKind::DeltaNet
        }
    }

    /// Bytes of boundary state a full-attention layer threads for `pos_n`
    /// positions on ONE side (K or V): `pos_n * kvd * 4` (int32 Q16 words).
    pub fn full_kv_side_bytes(&self, pos_n: u64) -> usize {
        (pos_n * self.kvd * 4) as usize
    }

    /// Bytes of the fixed-size DeltaNet snapshot a single DeltaNet layer threads
    /// at a boundary: conv state `(convK-1)*convDim*4` + recurrent S state
    /// `nVH*dK*dV*4`. For 35B: 98,304 + 2,097,152 = 2,195,456 B (~2.1 MB) — the
    /// reason [`stage_bounds`](Self::stage_bounds) avoids cutting mid-DeltaNet.
    pub fn delta_snapshot_bytes(&self) -> usize {
        (((self.conv_k.saturating_sub(1)) * self.conv_dim) * 4 + self.n_v_heads * self.d_k * self.d_v * 4)
            as usize
    }

    /// Plan the S contiguous layer stages of the prefill/decode pipeline.
    ///
    /// `n_layers` is taken from the per-run request (authoritative for the DAG),
    /// `requested_stages` from `InferRequest.stages`. For a pure-attention model
    /// (`full_attention_interval == None`, i.e. 0.6B) this is the original even
    /// split, byte-for-byte — so the proven path is unchanged.
    ///
    /// For a hybrid model with `align = true`, interior stage boundaries are
    /// SNAPPED to full-attention layer boundaries (multiples of the interval).
    /// Cutting there threads only the cheap KV slice (`kvd*4` per pos per side);
    /// cutting mid-DeltaNet-run would instead relay a ~2.1 MB S snapshot PER
    /// DeltaNet layer straddled. Snapping keeps every DeltaNet run whole inside a
    /// stage, so every boundary is a min-cost KV boundary. Falls back to the even
    /// split when there are too few full-attention boundaries to place `S-1`
    /// distinct cuts, or when snapping collides.
    pub fn stage_bounds(&self, n_layers: u64, requested_stages: u64, align: bool) -> Vec<(u64, u64)> {
        let n = n_layers.max(1);
        let s = requested_stages.clamp(1, n);
        let even = |s: u64| -> Vec<(u64, u64)> {
            (0..s).map(|i| (i * n / s, (i + 1) * n / s)).collect()
        };

        let Some(interval) = self.full_attention_interval else {
            return even(s);
        };
        if !align || s == 1 || interval == 0 {
            return even(s);
        }

        // Cheap interior boundaries: multiples of `interval` strictly inside (0, n)
        // — i.e. cuts that land right after a full-attention layer.
        let cheap: Vec<u64> = (1..(n / interval)).map(|m| m * interval).collect();
        if (s - 1) as usize > cheap.len() {
            return even(s); // not enough cheap boundaries to place S-1 cuts
        }

        let mut chosen: Vec<u64> = (1..s)
            .map(|i| {
                let target = i * n / s;
                *cheap
                    .iter()
                    .min_by_key(|&&b| (b as i64 - target as i64).abs())
                    .expect("cheap non-empty when s > 1 and enough boundaries")
            })
            .collect();
        chosen.sort_unstable();
        chosen.dedup();
        if chosen.len() != (s - 1) as usize {
            return even(s); // two targets snapped to the same boundary
        }

        let mut bounds = Vec::with_capacity(s as usize);
        let mut lo = 0u64;
        for b in chosen {
            bounds.push((lo, b));
            lo = b;
        }
        bounds.push((lo, n));
        bounds
    }

    /// Encode the per-segment `forwardRange` view-call calldata for this family.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_forward(&self, a: &ForwardArgs<'_>) -> Result<Vec<u8>> {
        match self.id {
            ModelId::Qwen3_06B => {
                // The proven 0.6B encoding — `Qwen3SegEngine.forwardRange` with a
                // KV-only `kvIn`. Unchanged from the original inline path.
                let call = forwardRangeCall {
                    rootDirectory: a.weights_root,
                    manifestHash: a.manifest,
                    packedConfig: a.packed_config,
                    q: SegCall {
                        span: SegSpan {
                            maxPos: U256::from(a.max_pos),
                            posLo: U256::from(a.pos_lo),
                            posHi: U256::from(a.pos_hi),
                            layerLo: U256::from(a.layer_lo),
                            layerHi: U256::from(a.layer_hi),
                        },
                        tokenIds: a.token_ids.to_vec(),
                        xIn: a.x_in.to_vec().into(),
                        kvIn: a.state_in.to_vec().into(),
                        expectXIn: keccak256(a.x_in),
                        expectKvIn: keccak256(a.state_in),
                    },
                };
                Ok(call.abi_encode())
            }
            ModelId::Qwen35 => {
                // `Qwen35SegEngine.forwardRange` — 4-word packedConfig + unified
                // `stateIn` (full-attention KV *and* DeltaNet snapshots). ABI per
                // the engine agent (commit 916ad17); pinned in the selector test.
                let call = seg35::forwardRangeCall {
                    rootDirectory: a.weights_root,
                    manifestHash: a.manifest,
                    packedConfig: self.packed_config4(a.packed_config, a.packed_config_w3)?,
                    q: seg35::Call35 {
                        span: seg35::Span35 {
                            maxPos: U256::from(a.max_pos),
                            posLo: U256::from(a.pos_lo),
                            posHi: U256::from(a.pos_hi),
                            layerLo: U256::from(a.layer_lo),
                            layerHi: U256::from(a.layer_hi),
                        },
                        tokenIds: a.token_ids.to_vec(),
                        xIn: a.x_in.to_vec().into(),
                        stateIn: a.state_in.to_vec().into(),
                        expectXIn: keccak256(a.x_in),
                        expectStateIn: keccak256(a.state_in),
                    },
                };
                Ok(call.abi_encode())
            }
        }
    }

    /// Encode the per-shard `argmaxRange` view-call calldata for this family.
    pub fn encode_argmax(&self, a: &ArgmaxArgs<'_>) -> Result<Vec<u8>> {
        match self.id {
            ModelId::Qwen3_06B => {
                let call = argmaxRangeCall {
                    rootDirectory: a.weights_root,
                    manifestHash: a.manifest,
                    packedConfig: a.packed_config,
                    xbFinal: a.xb_final.to_vec().into(),
                    vocabLo: U256::from(a.vocab_lo),
                    vocabHi: U256::from(a.vocab_hi),
                };
                Ok(call.abi_encode())
            }
            ModelId::Qwen35 => {
                let call = seg35::argmaxRangeCall {
                    rootDirectory: a.weights_root,
                    manifestHash: a.manifest,
                    packedConfig: self.packed_config4(a.packed_config, a.packed_config_w3)?,
                    xbFinal: a.xb_final.to_vec().into(),
                    vocabLo: U256::from(a.vocab_lo),
                    vocabHi: U256::from(a.vocab_hi),
                };
                Ok(call.abi_encode())
            }
        }
    }

    /// Decode a `forwardRange` return into `(xOut, stateAppend)`. The return
    /// SHAPE `(bytes, bytes, bytes32)` is identical across families, so this only
    /// differs in which ABI the words are named through.
    pub fn decode_forward_returns(&self, rd: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        match self.id {
            ModelId::Qwen3_06B => {
                let ret = forwardRangeCall::abi_decode_returns(rd)
                    .context("decoding forwardRange returndata")?;
                Ok((ret.xOut.to_vec(), ret.kvAppend.to_vec()))
            }
            ModelId::Qwen35 => {
                let ret = seg35::forwardRangeCall::abi_decode_returns(rd)
                    .context("decoding qwen35 forwardRange returndata")?;
                Ok((ret.xOut.to_vec(), ret.stateAppend.to_vec()))
            }
        }
    }

    /// Decode an `argmaxRange` return into `(bestScore, bestId)`. Return shape
    /// `(int256, uint256)` is identical across families; the merge rule (score
    /// desc, id asc) lives in the DAG planner and is model-agnostic.
    pub fn decode_argmax_returns(&self, rd: &[u8]) -> Result<(alloy_primitives::I256, u64)> {
        match self.id {
            ModelId::Qwen3_06B => {
                let ret = argmaxRangeCall::abi_decode_returns(rd)
                    .context("decoding argmaxRange returndata")?;
                Ok((ret.bestScore, ret.bestId.to::<u64>()))
            }
            ModelId::Qwen35 => {
                let ret = seg35::argmaxRangeCall::abi_decode_returns(rd)
                    .context("decoding qwen35 argmaxRange returndata")?;
                Ok((ret.bestScore, ret.bestId.to::<u64>()))
            }
        }
    }

    /// Assemble the 4-word packed config for the 35B ABI: the request's first
    /// three `bytes32` plus the required 4th word (`packed_config_w3`).
    fn packed_config4(&self, w012: [B256; 3], w3: Option<B256>) -> Result<[B256; 4]> {
        let w3 = w3.context(
            "qwen35 needs a 4-word packedConfig: set InferRequest.packed_config_w3 (SPEC §10 w3)",
        )?;
        Ok([w012[0], w012[1], w012[2], w3])
    }
}

/// Inputs to [`ModelSpec::encode_forward`], assembled by the DAG planner.
pub struct ForwardArgs<'a> {
    pub weights_root: alloy_primitives::Address,
    pub manifest: B256,
    pub packed_config: [B256; 3],
    pub packed_config_w3: Option<B256>,
    pub max_pos: u64,
    pub pos_lo: u64,
    pub pos_hi: u64,
    pub layer_lo: u64,
    pub layer_hi: u64,
    pub token_ids: &'a [u32],
    pub x_in: &'a [u8],
    pub state_in: &'a [u8],
}

/// Inputs to [`ModelSpec::encode_argmax`].
pub struct ArgmaxArgs<'a> {
    pub weights_root: alloy_primitives::Address,
    pub manifest: B256,
    pub packed_config: [B256; 3],
    pub packed_config_w3: Option<B256>,
    pub xb_final: &'a [u8],
    pub vocab_lo: u64,
    pub vocab_hi: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_kind_pattern_35b() {
        let s = ModelSpec::qwen35();
        // full-attention iff (l+1) % 4 == 0 → 3,7,11,...,39 (10 layers).
        let full: Vec<u64> = (0..40).filter(|&l| s.is_full_attention(l)).collect();
        assert_eq!(full, vec![3, 7, 11, 15, 19, 23, 27, 31, 35, 39]);
        assert_eq!(full.len(), 10);
        assert_eq!(s.layer_kind(0), LayerKind::DeltaNet);
        assert_eq!(s.layer_kind(3), LayerKind::Attention);
    }

    #[test]
    fn every_06b_layer_is_attention() {
        let s = ModelSpec::qwen3_06b();
        assert!((0..28).all(|l| s.is_full_attention(l)));
    }

    #[test]
    fn delta_snapshot_size_matches_spec() {
        // conv (convK-1)*convDim*4 + S nVH*dK*dV*4 = 98304 + 2097152.
        assert_eq!(ModelSpec::qwen35().delta_snapshot_bytes(), 2_195_456);
        assert_eq!(ModelSpec::qwen3_06b().delta_snapshot_bytes(), 0);
    }

    #[test]
    fn stage_bounds_06b_is_even_split() {
        let s = ModelSpec::qwen3_06b();
        // Unchanged from the original planner: even contiguous split.
        assert_eq!(s.stage_bounds(28, 2, true), vec![(0, 14), (14, 28)]);
        assert_eq!(s.stage_bounds(6, 3, true), vec![(0, 2), (2, 4), (4, 6)]);
        assert_eq!(s.stage_bounds(4, 1, true), vec![(0, 4)]);
    }

    #[test]
    fn stage_bounds_35b_snaps_to_full_attention_boundaries() {
        let s = ModelSpec::qwen35();
        // 2 stages: cut at 20 (after full-attention layer 19) — a KV-only cut.
        assert_eq!(s.stage_bounds(40, 2, true), vec![(0, 20), (20, 40)]);
        // 4 stages: all cuts land on multiples of 4 (after full layers 7,19,27).
        let b = s.stage_bounds(40, 4, true);
        for &(_, hi) in &b[..b.len() - 1] {
            assert_eq!(hi % 4, 0, "cut at {hi} is not a full-attention boundary");
        }
        assert_eq!(b.last().unwrap().1, 40);
    }

    #[test]
    fn stage_bounds_align_off_is_even_split() {
        let s = ModelSpec::qwen35();
        assert_eq!(s.stage_bounds(40, 3, false), vec![(0, 13), (13, 26), (26, 40)]);
    }

    #[test]
    fn from_env_defaults_to_06b() {
        // No env set in this test process → default.
        assert_eq!(ModelSpec::from_env().unwrap().id, ModelId::Qwen3_06B);
    }
}
