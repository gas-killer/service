//! Router-side shard coordinator: turns one LLM inference into a DAG of
//! hash-committed segment view-calls executed by k-of-N operator committees,
//! mirroring the proven reference driver (solidity-sdk
//! `src/examples/onchain-llm/tools/sharded_infer.py`) semantics exactly:
//! per-position prefill through S contiguous layer stages, sequential decode
//! relay, M-way vocab-sharded argmax merged (score desc, id asc), and
//! `Qwen3.generate`'s loop/stop semantics.
//!
//! Transport is the prewarm sidecar pattern: nodes poll `GET /shard/work`,
//! execute the pre-encoded `eth_call`, and `POST /shard/result`; the
//! coordinator advances a segment only when every committee member returned
//! byte-identical results. The assembled commit chain is served at
//! `GET /shard/chain/<pipelineRoot>` for the node validator gate.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::info;

use alloy_primitives::{Address, B256, I256, keccak256};

use gas_killer_common::shard::{
    ChainEntry, SegKind, ShardChain, ShardJob, ShardResultMsg, seg_chk,
};

use crate::model::{ArgmaxArgs, ForwardArgs, ModelSpec};

/// Per-segment results: (infer_id, seg_id) -> operator -> raw returndata.
type SegmentResults = HashMap<(String, u64), HashMap<u32, Vec<u8>>>;

/// A worker's advertised weight-shard capability, learned from its work-poll
/// query params (`common::shard::run_shard_loop`). Layer-affinity assignment
/// draws each segment's committee only from workers that COVER its layer span
/// (and hold the embedding / classifier stage when the segment needs it).
#[derive(Debug, Clone, Copy)]
pub struct WorkerCaps {
    pub layer_lo: u64,
    pub layer_hi: u64,
    pub has_embedding: bool,
    pub has_classifier: bool,
}

/// What a segment needs a worker to hold to be eligible for its committee.
#[derive(Debug, Clone, Copy)]
struct SegReq {
    layer_lo: u64,
    layer_hi: u64,
    needs_embedding: bool,
    needs_classifier: bool,
}

impl WorkerCaps {
    fn covers(&self, r: &SegReq) -> bool {
        // An empty span (layer_lo >= layer_hi) carries no layer requirement — used
        // by the classifier/argmax segment, which is gated on `has_classifier`
        // alone (the untied classifier lives on the last weight-shard slice, not
        // at a specific decoder-layer index).
        let span_ok = r.layer_lo >= r.layer_hi
            || (self.layer_lo <= r.layer_lo && self.layer_hi >= r.layer_hi);
        span_ok
            && (!r.needs_embedding || self.has_embedding)
            && (!r.needs_classifier || self.has_classifier)
    }
}

impl std::fmt::Display for SegReq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "layers [{}, {}){}{}",
            self.layer_lo,
            self.layer_hi,
            if self.needs_embedding {
                " +embedding"
            } else {
                ""
            },
            if self.needs_classifier {
                " +classifier"
            } else {
                ""
            },
        )
    }
}

/// `POST /shard/infer` request: everything the coordinator needs to plan and
/// verify one inference. All model facts are caller-supplied so the router
/// stays chain-agnostic; the trust anchor is that operators execute against
/// their OWN simulation env and the validator gate re-checks the chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferRequest {
    pub consumer: Address,
    pub seg_engine: Address,
    pub weights_root: Address,
    pub manifest: B256,
    pub packed_config: [B256; 3],
    /// 4th packed-config word, required only by the 35B (`qwen35`) ABI whose
    /// `packedConfig` is `bytes32[4]` (SPEC §10 w3). Unused/absent for 0.6B.
    #[serde(default)]
    pub packed_config_w3: Option<B256>,
    pub n_layers: u64,
    pub kvd: u64,
    pub dim: u64,
    pub vocab: u64,
    pub stop0: u32,
    pub stop1: u32,
    pub seq_cap: u64,
    pub prompt_ids: Vec<u32>,
    pub max_new: u64,
    #[serde(default = "default_stages")]
    pub stages: u64,
    #[serde(default = "default_argmax_shards")]
    pub argmax_shards: u64,
}

fn default_stages() -> u64 {
    2
}
fn default_argmax_shards() -> u64 {
    2
}

/// `POST /shard/infer` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferResponse {
    pub infer_id: String,
    pub answer_ids: Vec<u32>,
    pub pipeline_root: B256,
    pub segments: u64,
}

/// Shared coordinator state behind the internal HTTP endpoints.
pub struct ShardCoordinator {
    /// Committee size per segment (`GK_SHARD_K`, default 2).
    pub k: u64,
    /// Operator id space 0..N (`GK_SHARD_OPERATORS`, default 3). Only used by the
    /// legacy (non-weight-sharded) committee rotation; under layer-affinity the
    /// eligible set comes from the worker registry instead.
    pub n_operators: u64,
    /// Gas override for segment view calls (`GK_SHARD_GAS`, default 2^40).
    pub gas: u64,
    /// Model family the DAG planner is parameterized on (`GK_SHARD_MODEL`).
    pub spec: ModelSpec,
    /// Snap stage boundaries to full-attention layer boundaries for hybrid
    /// models (`GK_SHARD_ALIGN_STAGES`, default true). See [`ModelSpec::stage_bounds`].
    pub align_stages: bool,
    seq: AtomicU64,
    queues: Mutex<HashMap<u32, VecDeque<ShardJob>>>,
    results: Mutex<SegmentResults>,
    chains: Mutex<HashMap<B256, ShardChain>>,
    /// Live weight-shard registry keyed by operator id, populated by worker
    /// advertisements on the work poll. Empty => no weight-sharding => legacy
    /// unrestricted committee rotation (today's 0.6B behavior).
    workers: Mutex<HashMap<u32, WorkerCaps>>,
}

impl ShardCoordinator {
    pub fn from_env() -> Self {
        let env_u64 = |k: &str, d: u64| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d)
        };
        let spec = ModelSpec::from_env().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "shard: bad GK_SHARD_MODEL, defaulting to qwen3-0.6b");
            ModelSpec::qwen3_06b()
        });
        let align_stages = std::env::var("GK_SHARD_ALIGN_STAGES")
            .ok()
            .map(|v| !matches!(v.as_str(), "0" | "false" | "FALSE" | "no"))
            .unwrap_or(true);
        info!(
            model = spec.name,
            align_stages, "shard coordinator: model spec selected"
        );
        Self {
            k: env_u64("GK_SHARD_K", 2),
            n_operators: env_u64("GK_SHARD_OPERATORS", 3),
            gas: env_u64("GK_SHARD_GAS", 1 << 40),
            spec,
            align_stages,
            seq: AtomicU64::new(0),
            queues: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
            chains: Mutex::new(HashMap::new()),
            workers: Mutex::new(HashMap::new()),
        }
    }

    /// Records a worker's advertised weight-shard capability (`GET /shard/work`
    /// query params). Idempotent upsert keyed by operator id.
    pub fn advertise(&self, operator: u32, caps: WorkerCaps) {
        self.workers
            .lock()
            .expect("shard workers lock poisoned")
            .insert(operator, caps);
    }

    /// Drains the pending jobs for one operator (`GET /shard/work`).
    pub fn take_work(&self, operator: u32) -> Vec<ShardJob> {
        let mut queues = self.queues.lock().expect("shard queues lock poisoned");
        queues
            .get_mut(&operator)
            .map(|q| q.drain(..).collect())
            .unwrap_or_default()
    }

    /// Picks the k-of-N committee for one segment.
    ///
    /// - **No advertisements** (empty registry) => the legacy deterministic
    ///   rotation over the full `0..n_operators` space — byte-for-byte the
    ///   original behavior, so the 0.6B path is unchanged.
    /// - **Weight-sharded** (workers advertised slices) => the eligible set is
    ///   restricted to workers that COVER the segment (`WorkerCaps::covers`), then
    ///   the same deterministic rotation is applied *within* that set. Errors
    ///   clearly if fewer than `k` workers cover the span — a fleet misconfig.
    fn plan_committee(&self, unit: u64, seed: u64, req: &SegReq) -> Result<Vec<u32>> {
        let eligible: Option<Vec<u32>> = {
            let workers = self.workers.lock().expect("shard workers lock poisoned");
            if workers.is_empty() {
                None
            } else {
                let mut ops: Vec<u32> = workers
                    .iter()
                    .filter(|(_, c)| c.covers(req))
                    .map(|(&op, _)| op)
                    .collect();
                ops.sort_unstable();
                Some(ops)
            }
        };

        match eligible {
            None => Ok((0..self.k)
                .map(|j| ((seed + unit * self.k + j) % self.n_operators) as u32)
                .collect()),
            Some(ops) => {
                if (ops.len() as u64) < self.k {
                    bail!(
                        "layer-affinity: no {}-of committee covers segment {req} — only {} eligible \
                         worker(s) {ops:?} advertised coverage. Check GK_SHARD_LAYER_LO/HI and \
                         GK_SHARD_HAS_EMBEDDING/CLASSIFIER across the fleet (each covered span needs \
                         >= k workers).",
                        self.k,
                        ops.len(),
                    );
                }
                let n = ops.len() as u64;
                Ok((0..self.k)
                    .map(|j| ops[((seed + unit * self.k + j) % n) as usize])
                    .collect())
            }
        }
    }

    /// Accepts one executed segment result (`POST /shard/result`).
    pub fn put_result(&self, msg: ShardResultMsg) {
        let mut results = self.results.lock().expect("shard results lock poisoned");
        results
            .entry((msg.infer_id, msg.seg_id))
            .or_default()
            .insert(msg.operator, msg.returndata);
    }

    /// Serves the commit chain for a settled pipeline root (`GET /shard/chain/<root>`).
    pub fn chain(&self, root: B256) -> Option<ShardChain> {
        self.chains
            .lock()
            .expect("shard chains lock poisoned")
            .get(&root)
            .cloned()
    }

    /// Runs one full sharded inference to completion. Mirrors
    /// `sharded_infer.py::ShardedRunner.generate`.
    pub async fn run_inference(&self, req: InferRequest) -> Result<InferResponse> {
        let started = Instant::now();
        let infer_id = format!("inf-{}", self.seq.fetch_add(1, Ordering::SeqCst));
        let mut run = InferRun::new(self, &infer_id, &req)?;

        let p_len = req.prompt_ids.len() as u64;
        if p_len == 0 {
            bail!("empty prompt");
        }

        // Prefill: every prompt position through every stage (position-major,
        // sequential — the wavefront overlap is a latency optimization the
        // reference driver proves; correctness only needs the order).
        let mut last_x: Vec<u8> = Vec::new();
        for pos in 0..p_len {
            let mut x: Vec<u8> = Vec::new();
            for stage in 0..run.s {
                let toks = if stage == 0 {
                    vec![req.prompt_ids[pos as usize]]
                } else {
                    vec![]
                };
                x = run.forward_segment(stage, pos, pos + 1, toks, x).await?;
            }
            last_x = x;
        }

        // First generated token comes from the last prompt position's final
        // vector; then the decode relay, replaying Qwen3.generate's loop.
        let dim_bytes = (req.dim * 32) as usize;
        let xb = last_x[last_x.len() - dim_bytes..].to_vec();
        let mut generated: Vec<u32> = vec![run.argmax(xb).await?];
        let mut pos = p_len;
        while pos + 1 < run.max_pos
            && generated.last() != Some(&req.stop0)
            && generated.last() != Some(&req.stop1)
        {
            let token = *generated.last().expect("generated is non-empty");
            let mut x: Vec<u8> = Vec::new();
            for stage in 0..run.s {
                let toks = if stage == 0 { vec![token] } else { vec![] };
                x = run.forward_segment(stage, pos, pos + 1, toks, x).await?;
            }
            generated.push(run.argmax(x).await?);
            pos += 1;
        }

        let segments = run.entries.len() as u64;
        let mut chain = ShardChain {
            infer_id: infer_id.clone(),
            consumer: req.consumer,
            prompt_ids: req.prompt_ids.clone(),
            max_new: req.max_new,
            answer_ids: generated.clone(),
            pipeline_root: B256::ZERO,
            entries: run.entries,
        };
        chain.pipeline_root = chain.derive_root();
        let root = chain.pipeline_root;
        self.chains
            .lock()
            .expect("shard chains lock poisoned")
            .insert(root, chain);

        info!(
            infer_id = %infer_id,
            pipeline_root = %root,
            segments,
            answer_ids = ?generated,
            elapsed_s = started.elapsed().as_secs_f32(),
            "shard: inference complete — chain assembled"
        );
        Ok(InferResponse {
            infer_id,
            answer_ids: generated,
            pipeline_root: root,
            segments,
        })
    }

    /// Dispatches one segment to its (already-planned) committee and awaits
    /// byte-identical results.
    #[allow(clippy::too_many_arguments)]
    async fn exec_segment(
        &self,
        infer_id: &str,
        seg_id: u64,
        committee: Vec<u32>,
        kind: SegKind,
        to: Address,
        calldata: Vec<u8>,
    ) -> Result<(Vec<u8>, ChainEntry)> {
        {
            let mut queues = self.queues.lock().expect("shard queues lock poisoned");
            for &op in &committee {
                queues.entry(op).or_default().push_back(ShardJob {
                    infer_id: infer_id.to_string(),
                    seg_id,
                    kind,
                    to,
                    data: calldata.clone(),
                    gas: self.gas,
                });
            }
        }

        let deadline = Instant::now() + segment_timeout();
        let returns: HashMap<u32, Vec<u8>> = loop {
            {
                let results = self.results.lock().expect("shard results lock poisoned");
                if let Some(got) = results.get(&(infer_id.to_string(), seg_id))
                    && committee.iter().all(|op| got.contains_key(op))
                {
                    break got.clone();
                }
            }
            if Instant::now() > deadline {
                bail!(
                    "segment {seg_id} timed out waiting for committee {committee:?} \
                     (are the nodes running with GK_SHARD_URL set?)"
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        let first = &returns[&committee[0]];
        for &op in &committee[1..] {
            if &returns[&op] != first {
                bail!(
                    "COMMITTEE DIVERGENCE on segment {seg_id}: operators {committee:?} \
                     returned different results — aborting round"
                );
            }
        }
        let chk = seg_chk(kind, &calldata, first)?;
        info!(
            infer_id,
            seg_id,
            ?kind,
            ?committee,
            chk = %chk,
            "shard: committee agreed on segment"
        );
        Ok((
            first.clone(),
            ChainEntry {
                seg_id,
                kind,
                committee,
                chk,
                returndata_hash: keccak256(first),
            },
        ))
    }
}

/// Per-inference planning state (stage bounds, KV accumulators, commit chain).
struct InferRun<'a> {
    co: &'a ShardCoordinator,
    infer_id: String,
    req: &'a InferRequest,
    /// Effective stage count (requested, clamped to n_layers).
    s: u64,
    /// Stage layer bounds: bounds[i] = (layerLo, layerHi).
    bounds: Vec<(u64, u64)>,
    max_pos: u64,
    seed: u64,
    next_seg: u64,
    /// Per-layer full-attention KV accumulators (append semantics). For a
    /// pure-attention model (0.6B) every layer uses these — identical to before.
    k_acc: Vec<Vec<u8>>,
    v_acc: Vec<Vec<u8>>,
    /// Per-layer DeltaNet recurrent snapshot (conv || S), replace semantics — a
    /// fixed-size blob overwritten each segment (35B DeltaNet layers only; empty
    /// for attention layers).
    delta_acc: Vec<Vec<u8>>,
    entries: Vec<ChainEntry>,
}

impl<'a> InferRun<'a> {
    fn new(co: &'a ShardCoordinator, infer_id: &str, req: &'a InferRequest) -> Result<Self> {
        if req.n_layers == 0 || req.kvd == 0 || req.dim == 0 || req.vocab == 0 {
            bail!("bad model config");
        }
        // Stage bounds come from the model spec: even split for 0.6B (unchanged),
        // full-attention-boundary-aligned for the 35B hybrid (keeps costly
        // DeltaNet snapshots off stage cuts — see ModelSpec::stage_bounds).
        let bounds = co
            .spec
            .stage_bounds(req.n_layers, req.stages, co.align_stages);
        let s = bounds.len() as u64;
        let max_pos = (req.prompt_ids.len() as u64 + req.max_new).min(req.seq_cap);
        let seed_bytes = keccak256(format!("{infer_id}:{}", req.consumer).as_bytes());
        let seed = u64::from_be_bytes(seed_bytes[0..8].try_into().expect("8 bytes"));
        Ok(Self {
            co,
            infer_id: infer_id.to_string(),
            req,
            s,
            bounds,
            max_pos,
            seed,
            next_seg: 0,
            k_acc: vec![Vec::new(); req.n_layers as usize],
            v_acc: vec![Vec::new(); req.n_layers as usize],
            delta_acc: vec![Vec::new(); req.n_layers as usize],
            entries: Vec::new(),
        })
    }

    /// The resume boundary state for `stage`, layer-major per the wire format:
    /// full-attention layers contribute their accumulated `K || V` slices;
    /// DeltaNet layers contribute their current recurrent snapshot. For a
    /// pure-attention model this is exactly the original KV concatenation.
    fn state_in(&self, stage: u64) -> Vec<u8> {
        let (lo, hi) = self.bounds[stage as usize];
        let mut out = Vec::new();
        for l in lo..hi {
            if self.co.spec.is_full_attention(l) {
                out.extend_from_slice(&self.k_acc[l as usize]);
                out.extend_from_slice(&self.v_acc[l as usize]);
            } else {
                out.extend_from_slice(&self.delta_acc[l as usize]);
            }
        }
        out
    }

    /// Fold a segment's produced boundary state back into the accumulators:
    /// full-attention layers APPEND the new positions' `K`/`V`; DeltaNet layers
    /// REPLACE the running snapshot (it is position-count-defined, not additive).
    fn fold_state(&mut self, stage: u64, pos_n: u64, state_append: &[u8]) -> Result<()> {
        let (lo, hi) = self.bounds[stage as usize];
        let kv_side = self.co.spec.full_kv_side_bytes(pos_n); // per K or V side
        let snap = self.co.spec.delta_snapshot_bytes();

        let expected: usize = (lo..hi)
            .map(|l| {
                if self.co.spec.is_full_attention(l) {
                    2 * kv_side
                } else {
                    snap
                }
            })
            .sum();
        if state_append.len() != expected {
            bail!(
                "stateAppend length mismatch: {} bytes, expected {expected}",
                state_append.len()
            );
        }

        let mut at = 0usize;
        for l in lo..hi {
            let l = l as usize;
            if self.co.spec.is_full_attention(l as u64) {
                self.k_acc[l].extend_from_slice(&state_append[at..at + kv_side]);
                at += kv_side;
                self.v_acc[l].extend_from_slice(&state_append[at..at + kv_side]);
                at += kv_side;
            } else {
                // DeltaNet snapshot is "state after [0, posHi)" — replace wholesale.
                self.delta_acc[l] = state_append[at..at + snap].to_vec();
                at += snap;
            }
        }
        Ok(())
    }

    /// One forward segment on stage `stage`'s committee; returns xOut.
    async fn forward_segment(
        &mut self,
        stage: u64,
        pos_lo: u64,
        pos_hi: u64,
        token_ids: Vec<u32>,
        x_in: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let (lo, hi) = self.bounds[stage as usize];
        let state_in = self.state_in(stage);
        let calldata = self.co.spec.encode_forward(&ForwardArgs {
            weights_root: self.req.weights_root,
            manifest: self.req.manifest,
            packed_config: self.req.packed_config,
            packed_config_w3: self.req.packed_config_w3,
            max_pos: self.max_pos,
            pos_lo,
            pos_hi,
            layer_lo: lo,
            layer_hi: hi,
            token_ids: &token_ids,
            x_in: &x_in,
            state_in: &state_in,
        })?;
        // Layer-affinity: this segment covers layers [lo, hi); the embedding
        // (token lookup) happens when lo == 0, so that stage additionally needs a
        // worker advertising the embedding.
        let committee = self.co.plan_committee(
            stage,
            self.seed,
            &SegReq {
                layer_lo: lo,
                layer_hi: hi,
                needs_embedding: lo == 0,
                needs_classifier: false,
            },
        )?;
        let seg_id = self.next_seg;
        self.next_seg += 1;
        let (rd, entry) = self
            .co
            .exec_segment(
                &self.infer_id,
                seg_id,
                committee,
                SegKind::Forward,
                self.req.seg_engine,
                calldata,
            )
            .await?;
        self.entries.push(entry);
        let (x_out, state_append) = self.co.spec.decode_forward_returns(&rd)?;
        self.fold_state(stage, pos_hi - pos_lo, &state_append)?;
        Ok(x_out)
    }

    /// M-way vocab-sharded argmax merged (score desc, id asc); returns the token id.
    async fn argmax(&mut self, xb_final: Vec<u8>) -> Result<u32> {
        let m = self.req.argmax_shards.clamp(1, self.req.vocab);
        let step = self.req.vocab / m;
        let mut best: Option<(I256, u64)> = None;
        for j in 0..m {
            let (lo, hi) = (
                j * step,
                if j + 1 == m {
                    self.req.vocab
                } else {
                    (j + 1) * step
                },
            );
            let calldata = self.co.spec.encode_argmax(&ArgmaxArgs {
                weights_root: self.req.weights_root,
                manifest: self.req.manifest,
                packed_config: self.req.packed_config,
                packed_config_w3: self.req.packed_config_w3,
                xb_final: &xb_final,
                vocab_lo: lo,
                vocab_hi: hi,
            })?;
            // The classifier is untied and lives on the last weight-shard slice —
            // argmax shards go only to workers advertising it.
            let committee = self.co.plan_committee(
                self.s + j,
                self.seed,
                &SegReq {
                    layer_lo: 0,
                    layer_hi: 0,
                    needs_embedding: false,
                    needs_classifier: true,
                },
            )?;
            let seg_id = self.next_seg;
            self.next_seg += 1;
            let (rd, entry) = self
                .co
                .exec_segment(
                    &self.infer_id,
                    seg_id,
                    committee,
                    SegKind::Argmax,
                    self.req.seg_engine,
                    calldata,
                )
                .await?;
            self.entries.push(entry);
            let (score, id) = self.co.spec.decode_argmax_returns(&rd)?;
            best = Some(match best {
                None => (score, id),
                Some((bs, bi)) if score > bs || (score == bs && id < bi) => (score, id),
                Some(b) => b,
            });
        }
        let (_, id) = best.expect("m >= 1");
        u32::try_from(id).context("argmax id out of u32 range")
    }
}

/// Segment completion deadline (`GK_SHARD_SEGMENT_TIMEOUT_SECS`, default 180).
fn segment_timeout() -> Duration {
    let secs = std::env::var("GK_SHARD_SEGMENT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180);
    Duration::from_secs(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coordinator(k: u64, n: u64) -> ShardCoordinator {
        ShardCoordinator {
            k,
            n_operators: n,
            gas: 1 << 40,
            spec: ModelSpec::qwen3_06b(),
            align_stages: true,
            seq: AtomicU64::new(0),
            queues: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
            chains: Mutex::new(HashMap::new()),
            workers: Mutex::new(HashMap::new()),
        }
    }

    fn caps(lo: u64, hi: u64, emb: bool, cls: bool) -> WorkerCaps {
        WorkerCaps {
            layer_lo: lo,
            layer_hi: hi,
            has_embedding: emb,
            has_classifier: cls,
        }
    }

    #[test]
    fn work_queue_round_trip() {
        let co = coordinator(2, 3);
        co.queues
            .lock()
            .unwrap()
            .entry(1)
            .or_default()
            .push_back(ShardJob {
                infer_id: "inf-0".into(),
                seg_id: 0,
                kind: SegKind::Forward,
                to: Address::ZERO,
                data: vec![1, 2, 3],
                gas: 5,
            });
        assert_eq!(co.take_work(1).len(), 1);
        assert!(co.take_work(1).is_empty());
        assert!(co.take_work(0).is_empty());
    }

    #[test]
    fn committee_rotation_is_deterministic_and_disjointish() {
        // committee(unit) = (seed + unit*k + j) % n — two members always distinct for k=2, n=3
        let seed = 7u64;
        let (k, n) = (2u64, 3u64);
        for unit in 0..10u64 {
            let c: Vec<u64> = (0..k).map(|j| (seed + unit * k + j) % n).collect();
            assert_ne!(c[0], c[1]);
        }
    }

    #[tokio::test]
    async fn committee_divergence_aborts() {
        let co = coordinator(2, 3);
        let fut = co.exec_segment(
            "inf-0",
            0,
            vec![0, 1],
            SegKind::Argmax,
            Address::ZERO,
            vec![0xAA],
        );
        // committee {0, 1}; feed them different results
        co.put_result(ShardResultMsg {
            infer_id: "inf-0".into(),
            seg_id: 0,
            operator: 0,
            returndata: vec![1],
        });
        co.put_result(ShardResultMsg {
            infer_id: "inf-0".into(),
            seg_id: 0,
            operator: 1,
            returndata: vec![2],
        });
        let err = fut.await.expect_err("divergence must abort");
        assert!(err.to_string().contains("COMMITTEE DIVERGENCE"));
    }

    #[tokio::test]
    async fn committee_agreement_produces_entry() {
        let co = coordinator(2, 3);
        let fut = co.exec_segment(
            "inf-0",
            0,
            vec![0, 1],
            SegKind::Argmax,
            Address::ZERO,
            vec![0xAA],
        );
        for op in [0u32, 1u32] {
            co.put_result(ShardResultMsg {
                infer_id: "inf-0".into(),
                seg_id: 0,
                operator: op,
                returndata: vec![9, 9],
            });
        }
        let (rd, entry) = fut.await.expect("agreement");
        assert_eq!(rd, vec![9, 9]);
        assert_eq!(entry.committee, vec![0, 1]);
        assert_eq!(entry.returndata_hash, keccak256([9u8, 9u8]));
    }

    #[test]
    fn plan_committee_empty_registry_is_legacy_rotation() {
        // No advertisements => byte-for-byte the original modulo rotation over
        // the full operator space (the 0.6B path is unchanged).
        let co = coordinator(2, 3);
        let req = SegReq {
            layer_lo: 0,
            layer_hi: 14,
            needs_embedding: true,
            needs_classifier: false,
        };
        for unit in 0..10u64 {
            let c = co.plan_committee(unit, 7, &req).unwrap();
            let expect: Vec<u32> = (0..2).map(|j| ((7 + unit * 2 + j) % 3) as u32).collect();
            assert_eq!(c, expect);
        }
    }

    #[test]
    fn plan_committee_respects_layer_affinity() {
        // Two weight-shard groups: A holds [0,20)+embedding (ops 0,1),
        // B holds [20,40)+classifier (ops 2,3).
        let co = coordinator(2, 4);
        co.advertise(0, caps(0, 20, true, false));
        co.advertise(1, caps(0, 20, true, false));
        co.advertise(2, caps(20, 40, false, true));
        co.advertise(3, caps(20, 40, false, true));

        // A segment over layers [0,20) needing embedding must draw from {0,1}.
        let front = SegReq {
            layer_lo: 0,
            layer_hi: 20,
            needs_embedding: true,
            needs_classifier: false,
        };
        for unit in 0..8u64 {
            let c = co.plan_committee(unit, 3, &front).unwrap();
            assert!(
                c.iter().all(|op| *op == 0 || *op == 1),
                "front committee {c:?}"
            );
            assert_ne!(c[0], c[1]);
        }

        // A back segment over [20,40) must draw from {2,3}.
        let back = SegReq {
            layer_lo: 20,
            layer_hi: 40,
            needs_embedding: false,
            needs_classifier: false,
        };
        let c = co.plan_committee(0, 3, &back).unwrap();
        assert!(
            c.iter().all(|op| *op == 2 || *op == 3),
            "back committee {c:?}"
        );

        // Argmax needs the classifier — only group B qualifies.
        let cls = SegReq {
            layer_lo: 0,
            layer_hi: 0,
            needs_embedding: false,
            needs_classifier: true,
        };
        let c = co.plan_committee(5, 3, &cls).unwrap();
        assert!(
            c.iter().all(|op| *op == 2 || *op == 3),
            "classifier committee {c:?}"
        );
    }

    #[test]
    fn plan_committee_errors_when_span_uncovered() {
        // Only one worker covers a mid-range span; k=2 cannot be formed.
        let co = coordinator(2, 4);
        co.advertise(0, caps(0, 20, true, false));
        co.advertise(1, caps(0, 10, true, false)); // covers [0,10) only
        co.advertise(2, caps(20, 40, false, true));
        let req = SegReq {
            layer_lo: 10,
            layer_hi: 20,
            needs_embedding: false,
            needs_classifier: false,
        };
        let err = co.plan_committee(0, 0, &req).unwrap_err();
        assert!(err.to_string().contains("layer-affinity"), "{err}");
    }
}
