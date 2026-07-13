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

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, B256, U256, keccak256};

use gas_killer_common::shard::{
    ChainEntry, SegCall, SegKind, SegSpan, ShardChain, ShardJob, ShardResultMsg, argmaxRangeCall,
    forwardRangeCall, seg_chk,
};

/// Per-segment results: (infer_id, seg_id) -> operator -> raw returndata.
type SegmentResults = HashMap<(String, u64), HashMap<u32, Vec<u8>>>;

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
    /// Operator id space 0..N (`GK_SHARD_OPERATORS`, default 3).
    pub n_operators: u64,
    /// Gas override for segment view calls (`GK_SHARD_GAS`, default 2^40).
    pub gas: u64,
    seq: AtomicU64,
    queues: Mutex<HashMap<u32, VecDeque<ShardJob>>>,
    results: Mutex<SegmentResults>,
    chains: Mutex<HashMap<B256, ShardChain>>,
}

impl ShardCoordinator {
    pub fn from_env() -> Self {
        let env_u64 = |k: &str, d: u64| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d)
        };
        Self {
            k: env_u64("GK_SHARD_K", 2),
            n_operators: env_u64("GK_SHARD_OPERATORS", 3),
            gas: env_u64("GK_SHARD_GAS", 1 << 40),
            seq: AtomicU64::new(0),
            queues: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
            chains: Mutex::new(HashMap::new()),
        }
    }

    /// Drains the pending jobs for one operator (`GET /shard/work`).
    pub fn take_work(&self, operator: u32) -> Vec<ShardJob> {
        let mut queues = self.queues.lock().expect("shard queues lock poisoned");
        queues
            .get_mut(&operator)
            .map(|q| q.drain(..).collect())
            .unwrap_or_default()
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

    /// Dispatches one segment to its committee and awaits byte-identical results.
    #[allow(clippy::too_many_arguments)]
    async fn exec_segment(
        &self,
        infer_id: &str,
        seg_id: u64,
        unit: u64,
        seed: u64,
        kind: SegKind,
        to: Address,
        calldata: Vec<u8>,
    ) -> Result<(Vec<u8>, ChainEntry)> {
        let committee: Vec<u32> = (0..self.k)
            .map(|j| ((seed + unit * self.k + j) % self.n_operators) as u32)
            .collect();
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
    k_acc: Vec<Vec<u8>>,
    v_acc: Vec<Vec<u8>>,
    entries: Vec<ChainEntry>,
}

impl<'a> InferRun<'a> {
    fn new(co: &'a ShardCoordinator, infer_id: &str, req: &'a InferRequest) -> Result<Self> {
        if req.n_layers == 0 || req.kvd == 0 || req.dim == 0 || req.vocab == 0 {
            bail!("bad model config");
        }
        let s = req.stages.clamp(1, req.n_layers);
        let bounds = (0..s)
            .map(|i| (i * req.n_layers / s, (i + 1) * req.n_layers / s))
            .collect();
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
            entries: Vec::new(),
        })
    }

    fn kv_in(&self, stage: u64) -> Vec<u8> {
        let (lo, hi) = self.bounds[stage as usize];
        let mut out = Vec::new();
        for l in lo..hi {
            out.extend_from_slice(&self.k_acc[l as usize]);
            out.extend_from_slice(&self.v_acc[l as usize]);
        }
        out
    }

    fn fold_kv(&mut self, stage: u64, pos_n: u64, kv_append: &[u8]) -> Result<()> {
        let (lo, hi) = self.bounds[stage as usize];
        let blk = (pos_n * self.req.kvd * 4) as usize;
        if kv_append.len() != ((hi - lo) as usize) * 2 * blk {
            bail!("kvAppend length mismatch: {} bytes", kv_append.len());
        }
        for (j, l) in (lo..hi).enumerate() {
            let at = j * 2 * blk;
            self.k_acc[l as usize].extend_from_slice(&kv_append[at..at + blk]);
            self.v_acc[l as usize].extend_from_slice(&kv_append[at + blk..at + 2 * blk]);
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
        let kv_in = self.kv_in(stage);
        let call = forwardRangeCall {
            rootDirectory: self.req.weights_root,
            manifestHash: self.req.manifest,
            packedConfig: self.req.packed_config,
            q: SegCall {
                span: SegSpan {
                    maxPos: U256::from(self.max_pos),
                    posLo: U256::from(pos_lo),
                    posHi: U256::from(pos_hi),
                    layerLo: U256::from(lo),
                    layerHi: U256::from(hi),
                },
                tokenIds: token_ids,
                xIn: x_in.clone().into(),
                kvIn: kv_in.clone().into(),
                expectXIn: keccak256(&x_in),
                expectKvIn: keccak256(&kv_in),
            },
        };
        let seg_id = self.next_seg;
        self.next_seg += 1;
        let (rd, entry) = self
            .co
            .exec_segment(
                &self.infer_id,
                seg_id,
                stage,
                self.seed,
                SegKind::Forward,
                self.req.seg_engine,
                call.abi_encode(),
            )
            .await?;
        self.entries.push(entry);
        let ret = forwardRangeCall::abi_decode_returns(&rd)
            .context("decoding forwardRange returndata")?;
        self.fold_kv(stage, pos_hi - pos_lo, &ret.kvAppend)?;
        Ok(ret.xOut.to_vec())
    }

    /// M-way vocab-sharded argmax merged (score desc, id asc); returns the token id.
    async fn argmax(&mut self, xb_final: Vec<u8>) -> Result<u32> {
        let m = self.req.argmax_shards.clamp(1, self.req.vocab);
        let step = self.req.vocab / m;
        let mut best: Option<(alloy_primitives::I256, u64)> = None;
        for j in 0..m {
            let (lo, hi) = (
                j * step,
                if j + 1 == m {
                    self.req.vocab
                } else {
                    (j + 1) * step
                },
            );
            let call = argmaxRangeCall {
                rootDirectory: self.req.weights_root,
                manifestHash: self.req.manifest,
                packedConfig: self.req.packed_config,
                xbFinal: xb_final.clone().into(),
                vocabLo: U256::from(lo),
                vocabHi: U256::from(hi),
            };
            let seg_id = self.next_seg;
            self.next_seg += 1;
            let (rd, entry) = self
                .co
                .exec_segment(
                    &self.infer_id,
                    seg_id,
                    self.s + j,
                    self.seed,
                    SegKind::Argmax,
                    self.req.seg_engine,
                    call.abi_encode(),
                )
                .await?;
            self.entries.push(entry);
            let ret = argmaxRangeCall::abi_decode_returns(&rd)
                .context("decoding argmaxRange returndata")?;
            let (score, id) = (ret.bestScore, ret.bestId.to::<u64>());
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

// Silence unused-import lint for SolValue if the compiler decides it is unused
// in some cfg; abi encoding of tuples may route through it.
#[allow(unused)]
fn _assert_solvalue_in_scope() {
    let _ = <(U256,) as SolValue>::abi_encode;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coordinator(k: u64, n: u64) -> ShardCoordinator {
        ShardCoordinator {
            k,
            n_operators: n,
            gas: 1 << 40,
            seq: AtomicU64::new(0),
            queues: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
            chains: Mutex::new(HashMap::new()),
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
        let fut = co.exec_segment("inf-0", 0, 0, 0, SegKind::Argmax, Address::ZERO, vec![0xAA]);
        // committee for unit 0, seed 0 = {0, 1}; feed them different results
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
        let fut = co.exec_segment("inf-0", 0, 0, 0, SegKind::Argmax, Address::ZERO, vec![0xAA]);
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
}
