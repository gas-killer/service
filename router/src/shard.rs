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
    /// Model family for THIS request ("qwen3-0.6b" / "qwen35"). Absent =
    /// the coordinator's env-configured default (`GK_SHARD_MODEL`), so one
    /// router serves every pinned model without an env flip + restart.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_stages")]
    pub stages: u64,
    #[serde(default = "default_argmax_shards")]
    pub argmax_shards: u64,
    /// Opt-in prefix RESUME. When `Some(pl)`, the coordinator looks up the
    /// warmed prefix cached under `(model, keccak(prompt_ids[..pl]))` (see
    /// [`ShardCoordinator::warm_prefix`]), seeds every stage's boundary-state
    /// accumulators from it, and runs the batched prefill over only the NEW
    /// positions `[pl, prompt_ids.len())`. The prefix MUST have been warmed
    /// first (prefill-only) — a missing cache entry is an error, never a silent
    /// full recompute. Absent/`None` = a fresh from-scratch inference.
    #[serde(default)]
    pub prefix_len: Option<u64>,
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
    /// Set when the run RESUMED from a warmed prefix: the prefix pipeline root
    /// threaded from (this becomes `fulfilResumed`'s `prefixRoot`). `None` for a
    /// fresh run.
    #[serde(default)]
    pub prefix_root: Option<B256>,
}

/// `POST /shard/prefix` response — the committee-verified pipeline root of a
/// prefill-only prefix warm, plus the per-stage terminal state commitments a
/// later resume threads from. The `prefix_root` is what a subsequent
/// [`InferRequest`] carries implicitly (via `prefix_len`) and what
/// `fulfilResumed` binds as `prefixRoot`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefixWarmResponse {
    pub prefix_root: B256,
    pub prefix_len: u64,
    pub stage_state_commitments: Vec<B256>,
    pub segments: u64,
}

/// A stage's boundary-state accumulators captured at a prefix boundary — the
/// per-layer full-attention `K`/`V` (append semantics) and DeltaNet recurrent
/// snapshots (replace semantics), indexed by `layer - layer_lo`. Seeds a
/// resumed [`StageWorker`] so the resume's first forward segment threads from
/// exactly the prefix's terminal state.
#[derive(Debug, Clone)]
struct StageAccum {
    bounds: (u64, u64),
    k_acc: Vec<Vec<u8>>,
    v_acc: Vec<Vec<u8>>,
    delta_acc: Vec<Vec<u8>>,
}

impl StageAccum {
    /// The stage's boundary-state blob in the on-wire layer-major format (see
    /// [`StageWorker::state_in`]) and its `keccak256` commitment.
    fn state_bytes(&self, spec: &ModelSpec) -> Vec<u8> {
        let (lo, hi) = self.bounds;
        let mut out = Vec::new();
        for l in lo..hi {
            let i = (l - lo) as usize;
            if spec.is_full_attention(l) {
                out.extend_from_slice(&self.k_acc[i]);
                out.extend_from_slice(&self.v_acc[i]);
            } else {
                out.extend_from_slice(&self.delta_acc[i]);
            }
        }
        out
    }

    fn commitment(&self, spec: &ModelSpec) -> B256 {
        keccak256(self.state_bytes(spec))
    }
}

/// A warmed prefix held in the coordinator cache. The lineage invariant: this
/// was captured from a run that STOPPED at the prefix (prefill-only, no
/// decode) — a DeltaNet REPLACE snapshot cannot be truncated back once a later
/// position folds into it, so a run that decoded past the prefix can NEVER be
/// cached here.
#[derive(Debug, Clone)]
struct PrefixCacheEntry {
    prefix_ids: Vec<u32>,
    prefix_root: B256,
    stages: u64,
    bounds: Vec<(u64, u64)>,
    /// Per-stage terminal accumulators, indexed by stage — seed a resumed run.
    stage_accums: Vec<StageAccum>,
    /// Per-stage terminal state commitments (`keccak256(state_bytes)`) — the
    /// `expectStateIn` an honest resume threads from, and what the gate binds.
    stage_commitments: Vec<B256>,
}

/// Cache key: `(model_name, keccak(prompt_ids packed as 4-byte big-endian))`.
/// The packed hash mirrors the consumer's `keccak256(abi.encodePacked(promptIds))`.
fn prefix_key(model_name: &str, ids: &[u32]) -> (String, B256) {
    (model_name.to_string(), packed_ids_hash(ids))
}

/// `keccak256(abi.encodePacked(promptIds))` for a `uint32[]` — each id packed as
/// its 4-byte big-endian representation.
fn packed_ids_hash(ids: &[u32]) -> B256 {
    let mut buf = Vec::with_capacity(ids.len() * 4);
    for id in ids {
        buf.extend_from_slice(&id.to_be_bytes());
    }
    keccak256(&buf)
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
    /// Warmed-prefix cache keyed `(model_name, keccak(prefix_ids))`. Populated
    /// by [`ShardCoordinator::warm_prefix`] (a prefill-only run) and consumed by
    /// a resume ([`InferRequest::prefix_len`]).
    prefixes: Mutex<HashMap<(String, B256), PrefixCacheEntry>>,
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
            prefixes: Mutex::new(HashMap::new()),
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

    /// Warm a fixed prefix: a PREFILL-ONLY run over `[0, prompt_ids.len())` that
    /// captures each stage's TERMINAL boundary-state accumulators plus a
    /// committee-verified `prefixRoot`, caching them under
    /// `(model, keccak(prefix_ids))` for later resume (`POST /shard/prefix`).
    ///
    /// LINEAGE INVARIANT (do not violate): this run STOPS at the prefix — no
    /// decode, no argmax. A DeltaNet REPLACE snapshot cannot be truncated back
    /// once a later position folds into it, so only a run that halted exactly at
    /// the prefix may be cached as a resumable prefix.
    pub async fn warm_prefix(&self, req: InferRequest) -> Result<PrefixWarmResponse> {
        let started = Instant::now();
        let infer_id = format!("prefix-{}", self.seq.fetch_add(1, Ordering::SeqCst));
        let plan = InferPlan::new(self, &infer_id, &req)?;

        let p_len = req.prompt_ids.len() as u64;
        if p_len == 0 {
            bail!("empty prefix");
        }
        let s = plan.s;

        let cap = 2usize;
        let (tx0, mut rx_prev) = tokio::sync::mpsc::channel::<StageMsg>(cap);
        let mut worker_futs = Vec::with_capacity(s as usize);
        let mut rx_last = None;
        for stage in 0..s {
            let (tx_next, rx_next) = tokio::sync::mpsc::channel::<StageMsg>(cap);
            let worker = StageWorker::new(&plan, stage);
            let rx = std::mem::replace(&mut rx_prev, rx_next);
            worker_futs.push(worker.run(rx, tx_next));
            if stage + 1 == s {
                rx_last = Some(std::mem::replace(
                    &mut rx_prev,
                    tokio::sync::mpsc::channel::<StageMsg>(1).1,
                ));
            }
        }
        let mut rx_last = rx_last.expect("s >= 1");

        let driver = async {
            let tx0 = Some(tx0);
            // ONE batched prefill pass over the whole prefix [0, P). No decode.
            tx0.as_ref()
                .expect("tx0 alive")
                .send(StageMsg {
                    seg_base: 0,
                    pos_lo: 0,
                    pos_hi: p_len,
                    tokens: req.prompt_ids.clone(),
                    x: Vec::new(),
                })
                .await
                .map_err(|_| anyhow::anyhow!("stage pipeline closed unexpectedly"))?;
            // Drain the last stage's xOut (we never argmax it — prefill only).
            rx_last
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("pipeline ended during prefix prefill"))?;
            drop(tx0); // close pipeline; workers return (entries, terminal accum).
            Ok::<_, anyhow::Error>(())
        };

        let (worker_out, ()) =
            tokio::try_join!(futures::future::try_join_all(worker_futs), driver)?;

        // worker_out is stage-ordered (try_join_all preserves future order).
        let mut entries: Vec<ChainEntry> = Vec::new();
        let mut stage_accums: Vec<StageAccum> = Vec::with_capacity(s as usize);
        for (stage_entries, accum) in worker_out {
            entries.extend(stage_entries);
            stage_accums.push(accum);
        }
        entries.sort_by_key(|e| e.seg_id);
        for (i, e) in entries.iter().enumerate() {
            if e.seg_id != i as u64 {
                bail!(
                    "prefix chain assembly bug: segment ids not contiguous at {} (got {})",
                    i,
                    e.seg_id
                );
            }
        }
        let stage_commitments: Vec<B256> = stage_accums
            .iter()
            .map(|a| a.commitment(&plan.spec))
            .collect();

        let segments = entries.len() as u64;
        let mut chain = ShardChain {
            infer_id: infer_id.clone(),
            consumer: req.consumer,
            prompt_ids: req.prompt_ids.clone(),
            max_new: 0,
            answer_ids: Vec::new(),
            pipeline_root: B256::ZERO,
            entries,
            prefix_root: None,
            prefix_len: p_len,
            stage_state_commitments: stage_commitments.clone(),
        };
        chain.pipeline_root = chain.derive_root();
        let prefix_root = chain.pipeline_root;
        self.chains
            .lock()
            .expect("shard chains lock poisoned")
            .insert(prefix_root, chain);

        let entry = PrefixCacheEntry {
            prefix_ids: req.prompt_ids.clone(),
            prefix_root,
            stages: s,
            bounds: plan.bounds.clone(),
            stage_accums,
            stage_commitments: stage_commitments.clone(),
        };
        self.prefixes
            .lock()
            .expect("shard prefixes lock poisoned")
            .insert(prefix_key(plan.spec.name, &req.prompt_ids), entry);

        info!(
            infer_id = %infer_id,
            prefix_root = %prefix_root,
            prefix_len = p_len,
            segments,
            elapsed_s = started.elapsed().as_secs_f32(),
            "shard: prefix warmed — terminal per-stage state cached for resume"
        );
        Ok(PrefixWarmResponse {
            prefix_root,
            prefix_len: p_len,
            stage_state_commitments: stage_commitments,
            segments,
        })
    }

    /// Runs one full sharded inference to completion. Mirrors
    /// `sharded_infer.py::ShardedRunner.generate`. When `req.prefix_len` is set,
    /// RESUMES from a warmed prefix: each stage's accumulators are seeded from
    /// the cache and the batched prefill covers only the NEW positions
    /// `[prefix_len, P)` — the decode/argmax/chain arithmetic is otherwise
    /// identical, so a resumed run's answer is bit-exact with a fresh run over
    /// the same full prompt.
    pub async fn run_inference(&self, req: InferRequest) -> Result<InferResponse> {
        let started = Instant::now();
        let infer_id = format!("inf-{}", self.seq.fetch_add(1, Ordering::SeqCst));
        let plan = InferPlan::new(self, &infer_id, &req)?;

        let p_len = req.prompt_ids.len() as u64;
        if p_len == 0 {
            bail!("empty prompt");
        }
        let s = plan.s;
        let m = req.argmax_shards.clamp(1, req.vocab);

        // Resume seed: opt-in via req.prefix_len. Look up and validate the
        // warmed prefix so seeded accumulators line up with THIS plan.
        let resume: Option<PrefixCacheEntry> = match req.prefix_len {
            Some(pl) if pl > 0 => {
                if pl >= p_len {
                    bail!(
                        "resume: prefix_len {pl} >= prompt length {p_len} — nothing new to prefill \
                         (a resume must add at least one new token)"
                    );
                }
                let key = prefix_key(plan.spec.name, &req.prompt_ids[..pl as usize]);
                let entry = self
                    .prefixes
                    .lock()
                    .expect("shard prefixes lock poisoned")
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "resume: no warmed prefix cached for model {} at prefix_len {pl} — \
                             warm it via POST /shard/prefix first (opt-in resume never silently \
                             falls back to a full recompute)",
                            plan.spec.name
                        )
                    })?;
                if entry.prefix_ids.as_slice() != &req.prompt_ids[..pl as usize] {
                    bail!(
                        "resume: cached prefix ids differ from prompt_ids[..{pl}] \
                         (prefix_len mismatch or hash collision)"
                    );
                }
                if entry.stages != s || entry.bounds != plan.bounds {
                    bail!(
                        "resume: cached prefix stage layout (s={}, bounds={:?}) != current plan \
                         (s={s}, bounds={:?}) — re-warm with matching stages/n_layers",
                        entry.stages,
                        entry.bounds,
                        plan.bounds
                    );
                }
                Some(entry)
            }
            _ => None,
        };
        let start_pos = resume
            .as_ref()
            .map(|e| e.prefix_ids.len() as u64)
            .unwrap_or(0);

        // STAGE PIPELINE: one worker per contiguous-layer stage, connected by
        // channels. Each worker OWNS its layer range's boundary-state
        // accumulators (layers belong to exactly one stage) and records its
        // chain entries locally. On resume, a worker is SEEDED with its stage's
        // cached terminal accumulators so its first forward segment threads from
        // the prefix's terminal state (exactly the segment-to-segment threading
        // that already works between contiguous layer stages).
        //
        // BATCHED PREFILL: one pass — a single message (pos_lo=start_pos,
        // pos_hi=P) flows stage 0 -> 1 -> ... -> S-1, each running ONE
        // forwardRange over all NEW positions. Fresh: start_pos=0. Resume:
        // start_pos=prefix_len, so the prefix's positions are never re-executed.
        // Decode is inherently serial (token r needs argmax of r-1).
        //
        // Deterministic segment ids (coordinator and every operator agree, so
        // the commit chain re-derives identically):
        //   prefill stage s                   -> s
        //   argmax round 0 shard j            -> S + j
        //   decode round r>=1 forward stage s -> S + M + (r-1)*(S+M) + s
        //   decode round r>=1 argmax shard j  -> S + M + (r-1)*(S+M) + S + j
        let cap = 2usize;
        let (tx0, mut rx_prev) = tokio::sync::mpsc::channel::<StageMsg>(cap);
        let mut worker_futs = Vec::with_capacity(s as usize);
        let mut rx_last = None;
        for stage in 0..s {
            let (tx_next, rx_next) = tokio::sync::mpsc::channel::<StageMsg>(cap);
            let worker = match &resume {
                Some(e) => StageWorker::new_seeded(&plan, stage, &e.stage_accums[stage as usize]),
                None => StageWorker::new(&plan, stage),
            };
            let rx = std::mem::replace(&mut rx_prev, rx_next);
            worker_futs.push(worker.run(rx, tx_next));
            if stage + 1 == s {
                rx_last = Some(std::mem::replace(
                    &mut rx_prev,
                    tokio::sync::mpsc::channel::<StageMsg>(1).1,
                ));
            }
        }
        let mut rx_last = rx_last.expect("s >= 1");

        let driver = async {
            let mut tx0 = Some(tx0);
            let send = |tx0: &Option<tokio::sync::mpsc::Sender<StageMsg>>, msg: StageMsg| {
                let tx = tx0.as_ref().expect("tx0 alive while driving").clone();
                async move {
                    tx.send(msg)
                        .await
                        .map_err(|_| anyhow::anyhow!("stage pipeline closed unexpectedly"))
                }
            };

            // Prefill: ONE batched pass over the NEW positions [start_pos, P).
            send(
                &tx0,
                StageMsg {
                    seg_base: 0,
                    pos_lo: start_pos,
                    pos_hi: p_len,
                    tokens: req.prompt_ids[start_pos as usize..].to_vec(),
                    x: Vec::new(),
                },
            )
            .await?;
            let last_x = rx_last
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("pipeline ended during prefill"))?
                .x;

            // First generated token from the LAST prompt position's final
            // vector (the batched xOut carries all NEW positions; the last
            // dim-word slice is position P-1); then the decode relay.
            let dim_bytes = (req.dim * 32) as usize;
            let xb = last_x[last_x.len() - dim_bytes..].to_vec();
            let mut driver_entries = Vec::new();
            let (tok0, entries0) = plan.argmax_round(s, xb).await?;
            driver_entries.extend(entries0);
            let mut generated: Vec<u32> = vec![tok0];
            let mut pos = p_len;
            let mut round: u64 = 1;
            while pos + 1 < plan.max_pos
                && generated.last() != Some(&req.stop0)
                && generated.last() != Some(&req.stop1)
            {
                let token = *generated.last().expect("generated is non-empty");
                let seg_base = s + m + (round - 1) * (s + m);
                send(
                    &tx0,
                    StageMsg {
                        seg_base,
                        pos_lo: pos,
                        pos_hi: pos + 1,
                        tokens: vec![token],
                        x: Vec::new(),
                    },
                )
                .await?;
                let out = rx_last
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("pipeline ended during decode"))?;
                let (tok, entries) = plan.argmax_round(seg_base + s, out.x).await?;
                driver_entries.extend(entries);
                generated.push(tok);
                pos += 1;
                round += 1;
            }
            // Close the pipeline: workers drain and return their entries.
            tx0.take();
            Ok::<_, anyhow::Error>((generated, driver_entries))
        };

        let (worker_out, (generated, driver_entries)) =
            tokio::try_join!(futures::future::try_join_all(worker_futs), driver)?;

        let mut entries: Vec<ChainEntry> = worker_out
            .into_iter()
            .flat_map(|(stage_entries, _accum)| stage_entries)
            .chain(driver_entries)
            .collect();
        entries.sort_by_key(|e| e.seg_id);
        for (i, e) in entries.iter().enumerate() {
            if e.seg_id != i as u64 {
                bail!(
                    "chain assembly bug: segment ids not contiguous at {} (got {})",
                    i,
                    e.seg_id
                );
            }
        }

        let (prefix_root, prefix_len_field, stage_commitments) = match &resume {
            Some(e) => (
                Some(e.prefix_root),
                e.prefix_ids.len() as u64,
                e.stage_commitments.clone(),
            ),
            None => (None, 0, Vec::new()),
        };

        let segments = entries.len() as u64;
        let mut chain = ShardChain {
            infer_id: infer_id.clone(),
            consumer: req.consumer,
            prompt_ids: req.prompt_ids.clone(),
            max_new: req.max_new,
            answer_ids: generated.clone(),
            pipeline_root: B256::ZERO,
            entries,
            prefix_root,
            prefix_len: prefix_len_field,
            stage_state_commitments: stage_commitments,
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
            prefix_root = ?prefix_root,
            prefix_len = start_pos,
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
            prefix_root,
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
                let mut results = self.results.lock().expect("shard results lock poisoned");
                let key = (infer_id.to_string(), seg_id);
                if results
                    .get(&key)
                    .is_some_and(|got| committee.iter().all(|op| got.contains_key(op)))
                {
                    // Take ownership and EVICT: a 35B forward segment's
                    // returndata is ~10-20MB per committee member, and a full
                    // run has ~100 segments — retaining them all would hold
                    // gigabytes for the life of the process. Eviction bounds
                    // memory to the in-flight wavefront.
                    break results.remove(&key).expect("checked above");
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
                // Forward segments overwrite this with keccak256(state_in) in
                // StageWorker::forward_segment; argmax leaves it zero.
                expect_state_in: B256::ZERO,
            },
        ))
    }
}

/// A message flowing through the stage pipeline: one position's activation
/// entering (from the driver or the previous stage) or leaving a stage.
struct StageMsg {
    /// Deterministic id base for this pass: the segment executed by stage `s`
    /// for this message gets `seg_base + s`.
    seg_base: u64,
    /// Position range `[pos_lo, pos_hi)` this pass covers. Prefill batches the
    /// WHOLE prompt in one pass (`pos_lo=0, pos_hi=p_len`), so each stage runs a
    /// single `forwardRange` over all positions — amortizing each layer's weight
    /// read across the prompt instead of re-faulting the 34GB overlay once per
    /// position. Decode is inherently serial: one position per pass.
    pos_lo: u64,
    pos_hi: u64,
    /// Token ids for stage 0 (embedding); empty for later stages. One id per
    /// position in `[pos_lo, pos_hi)`.
    tokens: Vec<u32>,
    /// xIn for this stage (xOut of the previous, ALL positions); empty for stage 0.
    x: Vec<u8>,
}

/// Per-inference planning constants shared by the driver and all stage
/// workers (immutable during the run).
struct InferPlan<'a> {
    co: &'a ShardCoordinator,
    infer_id: String,
    req: &'a InferRequest,
    /// Per-request model spec (req.model, falling back to the coordinator's
    /// env default) — DAG shape and ABI encode/decode all hang off this.
    spec: ModelSpec,
    /// Effective stage count (requested, clamped to n_layers).
    s: u64,
    /// Stage layer bounds: bounds[i] = (layerLo, layerHi).
    bounds: Vec<(u64, u64)>,
    max_pos: u64,
    seed: u64,
}

impl<'a> InferPlan<'a> {
    fn new(co: &'a ShardCoordinator, infer_id: &str, req: &'a InferRequest) -> Result<Self> {
        if req.n_layers == 0 || req.kvd == 0 || req.dim == 0 || req.vocab == 0 {
            bail!("bad model config");
        }
        let spec = match &req.model {
            Some(name) => ModelSpec::from_name(name)?,
            None => co.spec.clone(),
        };
        // Stage bounds come from the model spec: even split for 0.6B (unchanged),
        // full-attention-boundary-aligned for the 35B hybrid (keeps costly
        // DeltaNet snapshots off stage cuts — see ModelSpec::stage_bounds).
        let bounds = spec.stage_bounds(req.n_layers, req.stages, co.align_stages);
        let s = bounds.len() as u64;
        let max_pos = (req.prompt_ids.len() as u64 + req.max_new).min(req.seq_cap);
        let seed_bytes = keccak256(format!("{infer_id}:{}", req.consumer).as_bytes());
        let seed = u64::from_be_bytes(seed_bytes[0..8].try_into().expect("8 bytes"));
        Ok(Self {
            co,
            infer_id: infer_id.to_string(),
            req,
            spec,
            s,
            bounds,
            max_pos,
            seed,
        })
    }

    /// M-way vocab-sharded argmax, shards dispatched CONCURRENTLY and merged
    /// (score desc, id asc — identical to the sequential merge); returns the
    /// token id and the shards' chain entries (seg ids `argmax_base + j`).
    async fn argmax_round(
        &self,
        argmax_base: u64,
        xb_final: Vec<u8>,
    ) -> Result<(u32, Vec<ChainEntry>)> {
        let m = self.req.argmax_shards.clamp(1, self.req.vocab);
        let step = self.req.vocab / m;
        let xb = &xb_final;
        let shard_futs = (0..m).map(|j| async move {
            let (lo, hi) = (
                j * step,
                if j + 1 == m {
                    self.req.vocab
                } else {
                    (j + 1) * step
                },
            );
            let calldata = self.spec.encode_argmax(&ArgmaxArgs {
                weights_root: self.req.weights_root,
                manifest: self.req.manifest,
                packed_config: self.req.packed_config,
                packed_config_w3: self.req.packed_config_w3,
                xb_final: xb,
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
            let (rd, entry) = self
                .co
                .exec_segment(
                    &self.infer_id,
                    argmax_base + j,
                    committee,
                    SegKind::Argmax,
                    self.req.seg_engine,
                    calldata,
                )
                .await?;
            let (score, id) = self.spec.decode_argmax_returns(&rd)?;
            Ok::<_, anyhow::Error>((score, id, entry))
        });
        let results = futures::future::try_join_all(shard_futs).await?;
        let mut best: Option<(I256, u64)> = None;
        let mut entries = Vec::with_capacity(results.len());
        for (score, id, entry) in results {
            entries.push(entry);
            best = Some(match best {
                None => (score, id),
                Some((bs, bi)) if score > bs || (score == bs && id < bi) => (score, id),
                Some(b) => b,
            });
        }
        let (_, id) = best.expect("m >= 1");
        Ok((
            u32::try_from(id).context("argmax id out of u32 range")?,
            entries,
        ))
    }
}

/// One pipeline stage's worker: owns its layer range's accumulators (each
/// layer belongs to exactly one stage) and executes its positions strictly in
/// arrival order — the recurrent-state chain fixes the per-stage order; the
/// wavefront parallelism is ACROSS stages.
struct StageWorker<'a> {
    co: &'a ShardCoordinator,
    req: &'a InferRequest,
    /// Per-request model spec (shared shape with InferPlan).
    spec: ModelSpec,
    infer_id: String,
    stage: u64,
    bounds: (u64, u64),
    max_pos: u64,
    seed: u64,
    /// Accumulators for THIS stage's layers, indexed by `layer - layerLo`:
    /// full-attention K/V (append semantics); DeltaNet recurrent snapshot
    /// (replace semantics). For a pure-attention model (0.6B) every layer
    /// uses K/V — identical to before.
    k_acc: Vec<Vec<u8>>,
    v_acc: Vec<Vec<u8>>,
    delta_acc: Vec<Vec<u8>>,
    entries: Vec<ChainEntry>,
}

impl<'a> StageWorker<'a> {
    fn new(plan: &InferPlan<'a>, stage: u64) -> Self {
        let bounds = plan.bounds[stage as usize];
        let n = (bounds.1 - bounds.0) as usize;
        Self {
            co: plan.co,
            req: plan.req,
            spec: plan.spec.clone(),
            infer_id: plan.infer_id.clone(),
            stage,
            bounds,
            max_pos: plan.max_pos,
            seed: plan.seed,
            k_acc: vec![Vec::new(); n],
            v_acc: vec![Vec::new(); n],
            delta_acc: vec![Vec::new(); n],
            entries: Vec::new(),
        }
    }

    /// Like [`Self::new`], but SEEDED with a warmed prefix's terminal
    /// accumulators for this stage (a resume). The first forward segment then
    /// threads from exactly this state — its `state_in()` blob (and hence its
    /// `expectStateIn` commitment) equals the cached prefix's stage commitment.
    fn new_seeded(plan: &InferPlan<'a>, stage: u64, accum: &StageAccum) -> Self {
        let mut w = Self::new(plan, stage);
        debug_assert_eq!(w.bounds, accum.bounds, "seeded accumulator bounds mismatch");
        w.k_acc = accum.k_acc.clone();
        w.v_acc = accum.v_acc.clone();
        w.delta_acc = accum.delta_acc.clone();
        w
    }

    /// Receives positions in order, executes this stage's segment for each,
    /// forwards xOut downstream. Returns the stage's chain entries AND its
    /// TERMINAL accumulators once the upstream closes. The terminal accumulators
    /// are what a prefix warm caches (and a resume ignores). Moving them out
    /// costs nothing; a normal run just drops them.
    async fn run(
        mut self,
        mut rx: tokio::sync::mpsc::Receiver<StageMsg>,
        tx: tokio::sync::mpsc::Sender<StageMsg>,
    ) -> Result<(Vec<ChainEntry>, StageAccum)> {
        while let Some(msg) = rx.recv().await {
            let x_out = self
                .forward_segment(
                    msg.seg_base + self.stage,
                    msg.pos_lo,
                    msg.pos_hi,
                    msg.tokens,
                    msg.x,
                )
                .await?;
            if tx
                .send(StageMsg {
                    seg_base: msg.seg_base,
                    pos_lo: msg.pos_lo,
                    pos_hi: msg.pos_hi,
                    tokens: Vec::new(),
                    x: x_out,
                })
                .await
                .is_err()
            {
                bail!("stage {}: downstream closed", self.stage);
            }
        }
        let accum = StageAccum {
            bounds: self.bounds,
            k_acc: self.k_acc,
            v_acc: self.v_acc,
            delta_acc: self.delta_acc,
        };
        Ok((self.entries, accum))
    }

    /// This stage's resume boundary state, layer-major per the wire format:
    /// full-attention layers contribute their accumulated `K || V` slices;
    /// DeltaNet layers contribute their current recurrent snapshot. For a
    /// pure-attention model this is exactly the original KV concatenation.
    fn state_in(&self) -> Vec<u8> {
        let (lo, hi) = self.bounds;
        let mut out = Vec::new();
        for l in lo..hi {
            let i = (l - lo) as usize;
            if self.spec.is_full_attention(l) {
                out.extend_from_slice(&self.k_acc[i]);
                out.extend_from_slice(&self.v_acc[i]);
            } else {
                out.extend_from_slice(&self.delta_acc[i]);
            }
        }
        out
    }

    /// Fold a segment's produced boundary state back into the accumulators:
    /// full-attention layers APPEND the new positions' `K`/`V`; DeltaNet layers
    /// REPLACE the running snapshot (it is position-count-defined, not additive).
    fn fold_state(&mut self, pos_n: u64, state_append: &[u8]) -> Result<()> {
        let (lo, hi) = self.bounds;
        // Per K or V side. The REQUEST's kvd is authoritative for the DAG (the
        // spec's kvd describes the default real model and diverges for e.g. the
        // CI fixture, whose kvd is 32 — caught by the fleet e2e).
        let kv_side = (pos_n * self.req.kvd * 4) as usize;
        let snap = self.spec.delta_snapshot_bytes();

        let expected: usize = (lo..hi)
            .map(|l| {
                if self.spec.is_full_attention(l) {
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
            let i = (l - lo) as usize;
            if self.spec.is_full_attention(l) {
                self.k_acc[i].extend_from_slice(&state_append[at..at + kv_side]);
                at += kv_side;
                self.v_acc[i].extend_from_slice(&state_append[at..at + kv_side]);
                at += kv_side;
            } else {
                // DeltaNet snapshot is "state after [0, posHi)" — replace wholesale.
                self.delta_acc[i] = state_append[at..at + snap].to_vec();
                at += snap;
            }
        }
        Ok(())
    }

    /// One forward segment (`seg_id` pre-assigned by the deterministic id
    /// formulas in `run_inference`) on this stage's committee; returns xOut.
    async fn forward_segment(
        &mut self,
        seg_id: u64,
        pos_lo: u64,
        pos_hi: u64,
        token_ids: Vec<u32>,
        x_in: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let (lo, hi) = self.bounds;
        let state_in = self.state_in();
        let calldata = self.spec.encode_forward(&ForwardArgs {
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
            self.stage,
            self.seed,
            &SegReq {
                layer_lo: lo,
                layer_hi: hi,
                needs_embedding: lo == 0,
                needs_classifier: false,
            },
        )?;
        let (rd, mut entry) = self
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
        // Record the commitment of the boundary state this segment threaded
        // FROM (matches the engine-verified `expectStateIn`). For a resume's
        // first forward segment this equals the cached prefix's stage terminal
        // commitment — the anchor the validator gate binds.
        entry.expect_state_in = keccak256(&state_in);
        self.entries.push(entry);
        let (x_out, state_append) = self.spec.decode_forward_returns(&rd)?;
        self.fold_state(pos_hi - pos_lo, &state_append)?;
        Ok(x_out)
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
            prefixes: Mutex::new(HashMap::new()),
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

    // ---- Prefix warm + resume, driven end-to-end through a mock EVM engine ----
    //
    // The mock is a faithful stand-in for the seg engine's boundary-state
    // threading: it maintains a per-position running carry chained over the
    // tokens seen so far, threaded FORWARD through the batched activation (x)
    // and ACROSS positions through the boundary state (KV append for attention
    // layers, snapshot REPLACE for DeltaNet layers) exactly as the real engine
    // composes segment-to-segment. That composition is what makes a resume
    // (warm `[0, pl)` then prefill `[pl, P)`) bit-exact with a fresh batched
    // prefill `[0, P)`: the carry at position P-1 is a pure function of the
    // token prefix `[0..P)` regardless of how the prefill was split.

    use crate::model::ModelId;
    use alloy::sol_types::SolCall;
    use alloy_primitives::{Bytes, U256};
    use gas_killer_common::shard::{ShardState, argmaxRangeCall, forwardRangeCall};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    /// Per-inference log of forward spans `(pos_lo, pos_hi, layer_lo)`, keyed by
    /// infer id — lets a test see exactly which positions a run executed.
    type ForwardLog = Arc<Mutex<HashMap<String, Vec<(u64, u64, u64)>>>>;

    fn coordinator_spec(k: u64, n: u64, spec: ModelSpec) -> ShardCoordinator {
        ShardCoordinator {
            k,
            n_operators: n,
            gas: 1 << 40,
            spec,
            align_stages: true,
            seq: AtomicU64::new(0),
            queues: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
            chains: Mutex::new(HashMap::new()),
            workers: Mutex::new(HashMap::new()),
            prefixes: Mutex::new(HashMap::new()),
        }
    }

    /// A tiny hybrid spec (2 DeltaNet + 2 full-attention layers) whose DeltaNet
    /// snapshot is 64 bytes — exercises the REPLACE-semantics boundary the
    /// lineage constraint is about, while staying fast. Uses the 0.6B call ABI
    /// (a container; layer semantics come from `full_attention_interval`).
    fn hybrid_spec() -> ModelSpec {
        ModelSpec {
            id: ModelId::Qwen3_06B,
            name: "qwen3-0.6b",
            n_layers: 4,
            full_attention_interval: Some(2),
            dim: 2,
            kvd: 8,
            vocab: 32,
            packed_config_words: 3,
            conv_dim: 0,
            conv_k: 1,
            n_v_heads: 1,
            d_k: 4,
            d_v: 4,
        }
    }

    fn mock_req(
        prompt: Vec<u32>,
        max_new: u64,
        prefix_len: Option<u64>,
        dim: u64,
        kvd: u64,
        vocab: u64,
        n_layers: u64,
    ) -> InferRequest {
        InferRequest {
            consumer: Address::from([7u8; 20]),
            seg_engine: Address::from([8u8; 20]),
            weights_root: Address::from([9u8; 20]),
            manifest: B256::ZERO,
            packed_config: [B256::ZERO; 3],
            packed_config_w3: None,
            n_layers,
            kvd,
            dim,
            vocab,
            stop0: u32::MAX,
            stop1: u32::MAX - 1,
            seq_cap: 4096,
            prompt_ids: prompt,
            max_new,
            model: None,
            stages: 2,
            argmax_shards: 1,
            prefix_len,
        }
    }

    /// Repeat a 32-byte word to fill `len` (32-aligned) bytes.
    fn fill(word: B256, len: usize) -> Vec<u8> {
        assert_eq!(len % 32, 0, "fill len must be 32-aligned");
        let mut v = Vec::with_capacity(len);
        for _ in 0..(len / 32) {
            v.extend_from_slice(word.as_slice());
        }
        v
    }

    fn mock_forward(
        spec: &ModelSpec,
        dim: u64,
        kvd: u64,
        job: &ShardJob,
        log: &ForwardLog,
    ) -> Vec<u8> {
        let call = forwardRangeCall::abi_decode(&job.data).expect("mock: decode forward calldata");
        let sp = &call.q.span;
        let pos_lo = sp.posLo.to::<u64>();
        let pos_hi = sp.posHi.to::<u64>();
        let layer_lo = sp.layerLo.to::<u64>();
        let layer_hi = sp.layerHi.to::<u64>();
        let tokens = &call.q.tokenIds;
        let x_in = call.q.xIn.as_ref();
        let state_in = call.q.kvIn.as_ref();
        let dim = dim as usize;
        let kvd = kvd as usize;
        let pos_n = (pos_hi - pos_lo) as usize;

        log.lock()
            .unwrap()
            .entry(job.infer_id.clone())
            .or_default()
            .push((pos_lo, pos_hi, layer_lo));

        // Carry threaded in from the boundary state = its last 32 bytes (each
        // layer's terminal contribution ends with the carry; empty = ZERO).
        let carry_in = if state_in.is_empty() {
            B256::ZERO
        } else {
            B256::from_slice(&state_in[state_in.len() - 32..])
        };

        let mut carries: Vec<B256> = Vec::with_capacity(pos_n);
        let mut c = carry_in;
        for p in 0..pos_n {
            let in_p: B256 = if layer_lo == 0 {
                keccak256(tokens[p].to_be_bytes())
            } else {
                B256::from_slice(&x_in[p * dim * 32..p * dim * 32 + 32])
            };
            let mut buf = Vec::with_capacity(80);
            buf.extend_from_slice(c.as_slice());
            buf.extend_from_slice(in_p.as_slice());
            buf.extend_from_slice(&layer_lo.to_be_bytes());
            buf.extend_from_slice(&layer_hi.to_be_bytes());
            c = keccak256(&buf);
            carries.push(c);
        }

        let mut x_out = Vec::with_capacity(pos_n * dim * 32);
        for &carry in &carries {
            x_out.extend_from_slice(&fill(carry, dim * 32));
        }

        // Boundary state in the coordinator's wire format (see fold_state):
        // attention layers append per-position K||V; DeltaNet layers REPLACE
        // their snapshot with the terminal carry.
        let snap = spec.delta_snapshot_bytes();
        let mut sa = Vec::new();
        for l in layer_lo..layer_hi {
            if spec.is_full_attention(l) {
                for &carry in &carries {
                    sa.extend_from_slice(&fill(carry, kvd * 4));
                }
                for &carry in &carries {
                    sa.extend_from_slice(&fill(carry, kvd * 4));
                }
            } else {
                sa.extend_from_slice(&fill(carries[pos_n - 1], snap));
            }
        }

        let mut chkbuf = Vec::with_capacity(x_out.len() + sa.len());
        chkbuf.extend_from_slice(&x_out);
        chkbuf.extend_from_slice(&sa);
        let chk = keccak256(&chkbuf);

        forwardRangeCall::abi_encode_returns_tuple(&(Bytes::from(x_out), Bytes::from(sa), chk))
    }

    fn mock_argmax(job: &ShardJob) -> Vec<u8> {
        let call = argmaxRangeCall::abi_decode(&job.data).expect("mock: decode argmax calldata");
        let xb = call.xbFinal.as_ref();
        let vocab_lo = call.vocabLo.to::<u64>();
        let vocab_hi = call.vocabHi.to::<u64>();
        // The final position's carry (= xOut[P-1]'s first word) picks the token.
        let carry = B256::from_slice(&xb[0..32]);
        let mut best: Option<(u64, u64)> = None;
        for id in vocab_lo..vocab_hi {
            let mut buf = [0u8; 40];
            buf[..32].copy_from_slice(carry.as_slice());
            buf[32..].copy_from_slice(&id.to_be_bytes());
            let h = keccak256(buf);
            let score = u64::from_be_bytes(h[0..8].try_into().unwrap());
            best = match best {
                None => Some((score, id)),
                Some((bs, bi)) if score > bs || (score == bs && id < bi) => Some((score, id)),
                Some(b) => Some(b),
            };
        }
        let (score, id) = best.expect("non-empty vocab range");
        argmaxRangeCall::abi_encode_returns_tuple(&(
            I256::try_from(U256::from(score)).unwrap(),
            U256::from(id),
        ))
    }

    async fn mock_engine(
        co: Arc<ShardCoordinator>,
        dim: u64,
        kvd: u64,
        log: ForwardLog,
        stop: Arc<AtomicBool>,
    ) {
        let spec = co.spec.clone();
        while !stop.load(Ordering::Relaxed) {
            let jobs = co.take_work(0);
            for job in jobs {
                let rd = match job.kind {
                    SegKind::Forward => mock_forward(&spec, dim, kvd, &job, &log),
                    SegKind::Argmax => mock_argmax(&job),
                };
                co.put_result(ShardResultMsg {
                    infer_id: job.infer_id,
                    seg_id: job.seg_id,
                    operator: 0,
                    returndata: rd,
                });
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    struct Harness {
        co: Arc<ShardCoordinator>,
        log: ForwardLog,
        stop: Arc<AtomicBool>,
        handle: tokio::task::JoinHandle<()>,
    }

    impl Harness {
        fn start(spec: ModelSpec, dim: u64, kvd: u64) -> Self {
            let co = Arc::new(coordinator_spec(1, 1, spec));
            let log: ForwardLog = Arc::new(Mutex::new(HashMap::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let handle = tokio::spawn(mock_engine(
                Arc::clone(&co),
                dim,
                kvd,
                Arc::clone(&log),
                Arc::clone(&stop),
            ));
            Self {
                co,
                log,
                stop,
                handle,
            }
        }

        async fn stop(self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = self.handle.await;
        }

        fn spans(&self, infer_id: &str) -> Vec<(u64, u64, u64)> {
            self.log
                .lock()
                .unwrap()
                .get(infer_id)
                .cloned()
                .unwrap_or_default()
        }
    }

    #[tokio::test]
    async fn resume_matches_non_resume_bit_exact() {
        let (dim, kvd, vocab, n_layers) = (2u64, 8u64, 32u64, 4u64);
        let h = Harness::start(ModelSpec::qwen3_06b(), dim, kvd);
        let prompt = vec![5u32, 9, 2, 7, 3];

        let fresh =
            h.co.run_inference(mock_req(prompt.clone(), 4, None, dim, kvd, vocab, n_layers))
                .await
                .expect("fresh inference");

        let warm =
            h.co.warm_prefix(mock_req(
                prompt[..3].to_vec(),
                0,
                None,
                dim,
                kvd,
                vocab,
                n_layers,
            ))
            .await
            .expect("prefix warm");
        let resume =
            h.co.run_inference(mock_req(
                prompt.clone(),
                4,
                Some(3),
                dim,
                kvd,
                vocab,
                n_layers,
            ))
            .await
            .expect("resumed inference");
        h.stop().await;

        assert!(
            !fresh.answer_ids.is_empty(),
            "sanity: some tokens generated"
        );
        assert_eq!(
            fresh.answer_ids, resume.answer_ids,
            "resume must produce the SAME answer ids as a fresh run over the full prompt"
        );
        assert_eq!(resume.prefix_root, Some(warm.prefix_root));
        assert_eq!(fresh.prefix_root, None);
    }

    #[tokio::test]
    async fn resume_skips_prefix_positions() {
        let (dim, kvd, vocab, n_layers) = (2u64, 8u64, 32u64, 4u64);
        let h = Harness::start(ModelSpec::qwen3_06b(), dim, kvd);
        let prompt = vec![1u32, 2, 3, 4, 5, 6];

        let fresh =
            h.co.run_inference(mock_req(prompt.clone(), 3, None, dim, kvd, vocab, n_layers))
                .await
                .expect("fresh");
        h.co.warm_prefix(mock_req(
            prompt[..4].to_vec(),
            0,
            None,
            dim,
            kvd,
            vocab,
            n_layers,
        ))
        .await
        .expect("warm");
        let resume =
            h.co.run_inference(mock_req(
                prompt.clone(),
                3,
                Some(4),
                dim,
                kvd,
                vocab,
                n_layers,
            ))
            .await
            .expect("resume");

        let resume_spans = h.spans(&resume.infer_id);
        let fresh_spans = h.spans(&fresh.infer_id);
        h.stop().await;

        // The resumed inference never re-executes any prefix position: its
        // earliest forward pos_lo is prefix_len (4), not 0.
        let resume_min = resume_spans.iter().map(|(lo, ..)| *lo).min().unwrap();
        assert_eq!(
            resume_min, 4,
            "resume must start prefill at prefix_len, skipping [0,4)"
        );
        let fresh_min = fresh_spans.iter().map(|(lo, ..)| *lo).min().unwrap();
        assert_eq!(fresh_min, 0, "fresh run prefills from position 0");

        // And it processes strictly fewer forward positions overall.
        let sum = |v: &[(u64, u64, u64)]| -> u64 { v.iter().map(|(lo, hi, _)| hi - lo).sum() };
        assert!(
            sum(&resume_spans) < sum(&fresh_spans),
            "resume forward positions {} should be fewer than fresh {}",
            sum(&resume_spans),
            sum(&fresh_spans)
        );
    }

    #[tokio::test]
    async fn resume_requires_a_warmed_prefix() {
        // Opt-in resume with no warmed prefix errors immediately (never a silent
        // full recompute) — no mock needed; it fails before dispatch.
        let co = coordinator_spec(1, 1, ModelSpec::qwen3_06b());
        let err = co
            .run_inference(mock_req(vec![1, 2, 3, 4], 2, Some(3), 2, 8, 32, 4))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no warmed prefix"), "{err}");
    }

    #[tokio::test]
    async fn resume_lineage_binds_coordinator_output_hybrid() {
        // Hybrid (DeltaNet REPLACE + attention APPEND) end-to-end: bit-exact
        // resume AND the node gate accepts the real coordinator-assembled chains.
        let (dim, kvd, vocab, n_layers) = (2u64, 8u64, 32u64, 4u64);
        let h = Harness::start(hybrid_spec(), dim, kvd);
        let prompt = vec![3u32, 1, 4, 1, 5, 9];

        let fresh =
            h.co.run_inference(mock_req(prompt.clone(), 3, None, dim, kvd, vocab, n_layers))
                .await
                .expect("fresh");
        let warm =
            h.co.warm_prefix(mock_req(
                prompt[..4].to_vec(),
                0,
                None,
                dim,
                kvd,
                vocab,
                n_layers,
            ))
            .await
            .expect("warm");
        let resume =
            h.co.run_inference(mock_req(
                prompt.clone(),
                3,
                Some(4),
                dim,
                kvd,
                vocab,
                n_layers,
            ))
            .await
            .expect("resume");
        let co = Arc::clone(&h.co);
        h.stop().await;

        assert_eq!(
            fresh.answer_ids, resume.answer_ids,
            "DeltaNet REPLACE snapshot must compose across the prefix split"
        );

        // The gate's lineage verifier accepts the coordinator's actual chains.
        let prefix_chain = co.chain(warm.prefix_root).expect("prefix chain served");
        let resumed_chain = co
            .chain(resume.pipeline_root)
            .expect("resumed chain served");
        assert_eq!(resumed_chain.prefix_root, Some(warm.prefix_root));
        assert_eq!(
            resumed_chain.stage_state_commitments, prefix_chain.stage_state_commitments,
            "resumed run threads from the prefix's terminal state commitments"
        );
        let node = ShardState::new(0, None, "http://router".into());
        node.verify_resume_lineage(&resumed_chain, &prefix_chain, warm.prefix_root)
            .expect("gate must accept honest coordinator-assembled resume lineage");
    }
}
