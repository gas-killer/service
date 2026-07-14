//! Sharded-inference segment channel: the router's shard coordinator splits one
//! LLM inference into hash-committed segments (`Qwen3SegEngine.forwardRange` /
//! `argmaxRange` view calls) and assigns each to a k-of-N operator committee.
//! Operator nodes poll `GET /shard/work?operator=<id>` (the same sidecar pattern
//! as the prewarm channel: nodes have no ingress, they poll the router's internal
//! server), execute the pre-encoded view call against their own simulation RPC,
//! and `POST /shard/result` with the raw returndata. All DAG logic — planning,
//! committee assignment, boundary-state threading, argmax merging, commit-chain
//! assembly — lives router-side; the node stays a dumb, cache-keeping executor.
//!
//! The trust hook is the VALIDATOR GATE: when a round's task targets the
//! configured sharded consumer (`GK_SHARD_CONSUMER`), the node refuses to sign
//! unless it can fetch the commit chain for the task's `pipelineRoot`, re-derive
//! the root from the segment commitments, confirm the answer ids, and confirm
//! that every segment it executed itself is present with exactly the digest it
//! computed locally. An operator signature over a sharded round therefore means
//! "I verified the chain, and I personally executed my committee's share of it".

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, keccak256};
use gas_analyzer::{EvmSketchExecutorCache, LocalStateCache, SimProfile, call_view_local_multi};

sol! {
    /// Mirror of `Qwen3Seg.Span` (solidity-sdk).
    struct SegSpan {
        uint256 maxPos;
        uint256 posLo;
        uint256 posHi;
        uint256 layerLo;
        uint256 layerHi;
    }

    /// Mirror of `Qwen3Seg.Call` (solidity-sdk).
    struct SegCall {
        SegSpan span;
        uint32[] tokenIds;
        bytes xIn;
        bytes kvIn;
        bytes32 expectXIn;
        bytes32 expectKvIn;
    }

    /// Mirror of `Qwen3SegEngine.forwardRange` (solidity-sdk).
    function forwardRange(
        address rootDirectory,
        bytes32 manifestHash,
        bytes32[3] packedConfig,
        SegCall q
    ) external view returns (bytes memory xOut, bytes memory kvAppend, bytes32 chk);

    /// Mirror of `Qwen3SegEngine.argmaxRange` (solidity-sdk).
    function argmaxRange(
        address rootDirectory,
        bytes32 manifestHash,
        bytes32[3] packedConfig,
        bytes memory xbFinal,
        uint256 vocabLo,
        uint256 vocabHi
    ) external view returns (int256 bestScore, uint256 bestId);

    /// Mirror of `GasKillerChatSharded.fulfil` (solidity-sdk) — the tracked,
    /// pure-commit settlement function whose calldata the validator gate decodes.
    /// Shared by the 0.6B (`GasKillerChatSharded`) and 35B
    /// (`GasKillerChat35Sharded`) consumers: identical signature/selector, only
    /// the CHAT_DOMAIN differs, so one gate decoder serves both.
    function fulfil(
        uint32[] promptIds,
        uint256 maxNewTokens,
        uint32[] answerIds,
        bytes32 pipelineRoot
    ) external;
}

/// Engine-v3 (Qwen3.5-35B-A3B, `Qwen35SegEngine`) segment ABI. Kept in its own
/// module because alloy's `sol!` derives the call struct name from the function
/// name (`forwardRange`/`argmaxRange`), so the 0.6B and 35B families would
/// otherwise collide. Two differences vs the 0.6B ABI above: (1) `packedConfig`
/// is `bytes32[4]` — SPEC §10 adds a 4th packed word; (2) the segment carries a
/// unified `stateIn`/`stateAppend` (full-attention KV **and** DeltaNet recurrent
/// snapshots), not the KV-only `kvIn`.
///
/// The return SHAPE is identical to the 0.6B forward — `(bytes, bytes, bytes32)`
/// — so `seg_chk(Forward, ..)` (third head word) is model-agnostic and needs no
/// change. Selectors are pinned in `common/tests/shard_selectors.rs` (forwardRange
/// 0x4faab046, argmaxRange 0x18d6ba7d — from the Qwen35Seg engine, commit 916ad17).
pub mod seg35 {
    use alloy::sol;

    sol! {
        /// Mirror of `Qwen35Seg.Span`.
        struct Span35 {
            uint256 maxPos;
            uint256 posLo;
            uint256 posHi;
            uint256 layerLo;
            uint256 layerHi;
        }

        /// Mirror of `Qwen35Seg.Call` — note `stateIn`/`expectStateIn` (unified
        /// full-attention KV + DeltaNet conv/S snapshots) replacing `kvIn`.
        struct Call35 {
            Span35 span;
            uint32[] tokenIds;
            bytes xIn;
            bytes stateIn;
            bytes32 expectXIn;
            bytes32 expectStateIn;
        }

        /// Mirror of `Qwen35SegEngine.forwardRange`.
        function forwardRange(
            address rootDirectory,
            bytes32 manifestHash,
            bytes32[4] packedConfig,
            Call35 q
        ) external view returns (bytes memory xOut, bytes memory stateAppend, bytes32 chk);

        /// Mirror of `Qwen35SegEngine.argmaxRange`.
        function argmaxRange(
            address rootDirectory,
            bytes32 manifestHash,
            bytes32[4] packedConfig,
            bytes memory xbFinal,
            uint256 vocabLo,
            uint256 vocabHi
        ) external view returns (int256 bestScore, uint256 bestId);
    }
}

/// Segment kind, mirrored in job/chain records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegKind {
    Forward,
    Argmax,
}

/// One unit of work for an operator: a pre-encoded view call. The node never
/// interprets the model — it executes `eth_call{to, data, gas}` and returns the
/// raw returndata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardJob {
    pub infer_id: String,
    pub seg_id: u64,
    pub kind: SegKind,
    pub to: Address,
    #[serde(with = "hex_bytes")]
    pub data: Vec<u8>,
    pub gas: u64,
}

/// `GET /shard/work?operator=<id>` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardWork {
    pub jobs: Vec<ShardJob>,
}

/// The pinned local-execution environment segment view calls run through
/// under `GK_SIM_EXECUTOR=local` — built by
/// [`crate::GasKillerValidator::local_view_ctx`] from the validator's OWN
/// caches and config, so a segment executes in exactly the environment the
/// tracked-function path uses: same memoized manifest-verified mmap overlay
/// mounts, same pinned [`SimProfile`], same lazy remote-state backend.
///
/// This is what makes phantom-overlay models (the ~34GB Qwen3.5-35B-A3B)
/// servable as segments at all: their chunks exist on no RPC node, so a
/// plain `eth_call` reads empty code — the overlay must mount in-process.
#[derive(Clone)]
pub struct LocalViewCtx {
    pub executor_cache: Arc<EvmSketchExecutorCache>,
    pub local_state_cache: Arc<LocalStateCache>,
    pub sim_profile: SimProfile,
    /// One `(weights_path, tokenizer_path, manifest)` spec per pinned model
    /// (`GK_OVERLAY_WEIGHTS[_N]`/...). Empty mounts nothing — segments then
    /// see only base chain state, correct for consumers without overlays.
    pub overlay_files: Vec<(String, String, B256)>,
}

/// `POST /shard/result` body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardResultMsg {
    pub infer_id: String,
    pub seg_id: u64,
    pub operator: u32,
    #[serde(with = "hex_bytes")]
    pub returndata: Vec<u8>,
}

/// One link of the commit chain, as served at `GET /shard/chain/<root>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainEntry {
    pub seg_id: u64,
    pub kind: SegKind,
    pub committee: Vec<u32>,
    pub chk: B256,
    pub returndata_hash: B256,
}

/// The full commit chain for one sharded inference. `pipeline_root` is the
/// keccak fold of every entry's `chk` in `seg_id` order — exactly what the
/// consumer's `fulfil` calldata carries and what the validator gate re-derives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardChain {
    pub infer_id: String,
    pub consumer: Address,
    pub prompt_ids: Vec<u32>,
    pub max_new: u64,
    pub answer_ids: Vec<u32>,
    pub pipeline_root: B256,
    pub entries: Vec<ChainEntry>,
}

impl ShardChain {
    /// Re-derives the pipeline root from the entries (keccak over the
    /// concatenated segment commitments in seg_id order).
    pub fn derive_root(&self) -> B256 {
        let mut sorted: Vec<&ChainEntry> = self.entries.iter().collect();
        sorted.sort_by_key(|e| e.seg_id);
        let mut buf = Vec::with_capacity(sorted.len() * 32);
        for e in &sorted {
            buf.extend_from_slice(e.chk.as_slice());
        }
        keccak256(&buf)
    }
}

/// What a node remembers about a segment it executed: enough to later verify
/// the router-assembled chain against its own computation.
#[derive(Debug, Clone, Copy)]
pub struct ExecutedSeg {
    pub returndata_hash: B256,
    pub chk: B256,
}

/// Node-side shard state, shared between the poll/execute loop (writer) and the
/// validator gate (reader).
#[derive(Debug)]
pub struct ShardState {
    /// This operator's id in the coordinator's committee space (0..N).
    pub operator_id: u32,
    /// Optional sharded-consumer allowlist. `Some(addr)` gates only rounds
    /// targeting that address; `None` gates any round whose calldata is a
    /// `fulfil(...)` call — which lets sharding be enabled at node startup
    /// (before the consumer is deployed), so the p2p mesh forms once instead
    /// of being torn by a mid-run node recreation.
    pub consumer: Option<Address>,
    /// Base URL of the router's internal server, e.g. `http://router:8081`.
    pub router_base: String,
    /// Weight-sharding capability advertisement. `layer_range = Some((lo, hi))`
    /// means this worker HOLDS only layers `[lo, hi)` of the model (a
    /// weight-shard slice) and may execute only segments whose layer span it
    /// covers; `None` (the default) means "full model" — today's behavior, no
    /// advertisement sent, so the router falls back to unrestricted committee
    /// rotation. `has_embedding`/`has_classifier` advertise the embedding
    /// (layer-0) stage and the vocab-classifier (argmax) capability, which for a
    /// weight-shard fleet live only on the first/last slice respectively.
    pub layer_range: Option<(u64, u64)>,
    pub has_embedding: bool,
    pub has_classifier: bool,
    executed: Mutex<HashMap<(String, u64), ExecutedSeg>>,
}

impl ShardState {
    pub fn new(operator_id: u32, consumer: Option<Address>, router_base: String) -> Self {
        Self {
            operator_id,
            consumer,
            router_base,
            layer_range: None,
            has_embedding: false,
            has_classifier: false,
            executed: Mutex::new(HashMap::new()),
        }
    }

    /// Advertise this worker's held weight-shard slice (env
    /// `GK_SHARD_LAYER_LO`/`GK_SHARD_LAYER_HI`/`GK_SHARD_HAS_EMBEDDING`/
    /// `GK_SHARD_HAS_CLASSIFIER`). The values are appended to the work-poll query
    /// so the router's layer-affinity planner learns the fleet layout without a
    /// separate registration round-trip.
    pub fn with_capabilities(
        mut self,
        layer_range: Option<(u64, u64)>,
        has_embedding: bool,
        has_classifier: bool,
    ) -> Self {
        self.layer_range = layer_range;
        self.has_embedding = has_embedding;
        self.has_classifier = has_classifier;
        self
    }

    /// Whether this task is a sharded-settlement round this node must gate:
    /// calldata is a `fulfil(...)` call and (when a consumer allowlist is
    /// configured) it targets that consumer.
    pub fn gates(&self, target: Address, call_data: &[u8]) -> bool {
        let is_fulfil = call_data.len() >= 4 && call_data[0..4] == fulfilCall::SELECTOR;
        is_fulfil && self.consumer.map(|c| c == target).unwrap_or(true)
    }

    pub fn record(&self, infer_id: &str, seg_id: u64, seg: ExecutedSeg) {
        self.executed
            .lock()
            .expect("shard executed lock poisoned")
            .insert((infer_id.to_string(), seg_id), seg);
    }

    pub fn executed(&self, infer_id: &str, seg_id: u64) -> Option<ExecutedSeg> {
        self.executed
            .lock()
            .expect("shard executed lock poisoned")
            .get(&(infer_id.to_string(), seg_id))
            .copied()
    }

    /// The validator gate. Decodes `fulfil` calldata, fetches the commit chain
    /// for its pipeline root from the router, and verifies:
    /// 1. the chain's derived root matches both the served and calldata roots,
    /// 2. the chain's answer/prompt ids match the calldata,
    /// 3. every entry whose committee includes this operator matches the
    ///    digest this node computed when it executed that segment itself,
    /// 4. this node actually executed at least one segment of the chain.
    ///
    /// Any failure returns Err — the caller must refuse to sign.
    pub async fn verify_fulfil_task(&self, call_data: &[u8]) -> Result<()> {
        let call = fulfilCall::abi_decode(call_data).context(
            "shard gate: task targets the sharded consumer but calldata is not fulfil()",
        )?;
        let root = call.pipelineRoot;
        if root == B256::ZERO {
            bail!("shard gate: zero pipeline root");
        }

        let url = format!("{}/shard/chain/{root}", self.router_base);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("shard gate: building HTTP client")?;
        let chain: ShardChain = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("shard gate: GET {url}"))?
            .error_for_status()
            .with_context(|| format!("shard gate: GET {url}"))?
            .json()
            .await
            .context("shard gate: decoding chain")?;

        let derived = chain.derive_root();
        if derived != root || chain.pipeline_root != root {
            bail!(
                "shard gate: root mismatch (calldata {root}, served {}, derived {derived})",
                chain.pipeline_root
            );
        }
        if chain.answer_ids != call.answerIds {
            bail!(
                "shard gate: answer ids mismatch (chain {:?}, calldata {:?})",
                chain.answer_ids,
                call.answerIds
            );
        }
        if chain.prompt_ids != call.promptIds {
            bail!("shard gate: prompt ids mismatch");
        }

        let mut own = 0usize;
        for entry in &chain.entries {
            if !entry.committee.contains(&self.operator_id) {
                continue;
            }
            let Some(mine) = self.executed(&chain.infer_id, entry.seg_id) else {
                bail!(
                    "shard gate: chain claims I executed segment {} of {} but I have no record of it",
                    entry.seg_id,
                    chain.infer_id
                );
            };
            if mine.returndata_hash != entry.returndata_hash || mine.chk != entry.chk {
                bail!(
                    "shard gate: segment {} digest mismatch (mine chk {} vs chain {})",
                    entry.seg_id,
                    mine.chk,
                    entry.chk
                );
            }
            own += 1;
        }
        if own == 0 {
            bail!(
                "shard gate: I executed no segment of pipeline {root} — refusing to attest work I never saw"
            );
        }
        info!(
            pipeline_root = %root,
            own_segments = own,
            total_segments = chain.entries.len(),
            "shard gate: verified commit chain — my {own} executed segments match, root and answer ids check out"
        );
        Ok(())
    }
}

/// Extracts the segment commitment from a job's raw returndata. `forwardRange`
/// returns `(bytes, bytes, bytes32)`, so `chk` is the third head word; argmax
/// segments get a synthetic commitment binding the job calldata to its result.
pub fn seg_chk(kind: SegKind, job_data: &[u8], returndata: &[u8]) -> Result<B256> {
    match kind {
        SegKind::Forward => {
            if returndata.len() < 96 {
                bail!("forward returndata too short: {}", returndata.len());
            }
            Ok(B256::from_slice(&returndata[64..96]))
        }
        SegKind::Argmax => {
            let mut buf = Vec::with_capacity(24 + job_data.len() + returndata.len());
            buf.extend_from_slice(b"gaskiller.seg.argmax.v1");
            buf.extend_from_slice(keccak256(job_data).as_slice());
            buf.extend_from_slice(returndata);
            Ok(keccak256(&buf))
        }
    }
}

/// Poll interval for the node's shard loop (`GK_SHARD_POLL_MS`, default 500).
fn shard_poll_interval() -> Duration {
    let ms = std::env::var("GK_SHARD_POLL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&ms| ms > 0)
        .unwrap_or(500);
    Duration::from_millis(ms)
}

/// Executes one pre-encoded view call via plain JSON-RPC `eth_call` with an
/// explicit gas override (segments run under the lifted simulation gas budget,
/// exactly like tracked-function analysis; they are `view`, so no state risk).
async fn eth_call(client: &reqwest::Client, rpc_url: &str, job: &ShardJob) -> Result<Vec<u8>> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_call",
        "params": [{
            "to": format!("{}", job.to),
            "data": format!("0x{}", hex::encode(&job.data)),
            "gas": format!("0x{:x}", job.gas),
        }, "latest"],
    });
    let resp: serde_json::Value = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("eth_call POST {rpc_url}"))?
        .error_for_status()?
        .json()
        .await
        .context("eth_call: decoding JSON-RPC response")?;
    if let Some(err) = resp.get("error") {
        bail!("eth_call error: {err}");
    }
    let hex_str = resp
        .get("result")
        .and_then(|r| r.as_str())
        .context("eth_call: missing result")?;
    hex::decode(hex_str.trim_start_matches("0x")).context("eth_call: result not hex")
}

/// Fetches the latest block number from `rpc_url` — the pin for the local
/// executor's block-scoped state backend. Segment view calls read only
/// immutable code (the deployed seg engine + the mounted overlay), so ANY
/// recent block yields byte-identical output across committee members; the
/// pin exists because the analyzer's caches are block-keyed.
async fn eth_block_number(client: &reqwest::Client, rpc_url: &str) -> Result<u64> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_blockNumber",
        "params": [],
    });
    let resp: serde_json::Value = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("eth_blockNumber POST {rpc_url}"))?
        .error_for_status()?
        .json()
        .await
        .context("eth_blockNumber: decoding JSON-RPC response")?;
    if let Some(err) = resp.get("error") {
        bail!("eth_blockNumber error: {err}");
    }
    let hex_str = resp
        .get("result")
        .and_then(|r| r.as_str())
        .context("eth_blockNumber: missing result")?;
    u64::from_str_radix(hex_str.trim_start_matches("0x"), 16)
        .context("eth_blockNumber: result not hex")
}

/// Executes one pre-encoded segment view call IN-PROCESS through the
/// analyzer's local executor with the validator's pinned overlay mounts
/// (`GK_SIM_EXECUTOR=local`), returning the raw returndata — the local twin
/// of [`eth_call`]. The RPC serves only as the lazy base-state backend (the
/// deployed seg engine's code); the model weights come from the mmap mounts.
///
/// Safe to await directly on the shared 2-worker runtime:
/// `call_view_local_multi` runs its entire blocking half — the segment's
/// minutes of revm compute AND the first call's one-time streaming-keccak
/// mount verification over the full artifact files — on the blocking pool
/// (the analyzer takes the state cache as an `Arc` precisely for this), so
/// /healthz and P2P never starve (the PR#319 incident class).
async fn local_view_call(
    client: &reqwest::Client,
    rpc_url: &str,
    ctx: &LocalViewCtx,
    job: &ShardJob,
) -> Result<Vec<u8>> {
    let block_number = eth_block_number(client, rpc_url).await?;
    let tx_request = TransactionRequest::default()
        .to(job.to)
        .input(alloy_primitives::Bytes::copy_from_slice(&job.data).into())
        .gas_limit(job.gas);
    call_view_local_multi(
        &ctx.executor_cache,
        Arc::clone(&ctx.local_state_cache),
        rpc_url,
        tx_request,
        block_number,
        ctx.sim_profile,
        &ctx.overlay_files,
    )
    .await
    .map(|bytes| bytes.to_vec())
}

/// Runs forever: polls the router's shard work queue for this operator,
/// executes each job — in-process through the analyzer's local executor with
/// the pinned overlay mounts when `view_ctx` is set (`GK_SIM_EXECUTOR=local`),
/// else as plain `eth_call` against `rpc_url` — records the digests locally
/// (for the validator gate), and posts the raw returndata back to the router.
///
/// Intended to be spawned on the node when `GK_SHARD_URL` is set.
pub async fn run_shard_loop(
    state: std::sync::Arc<ShardState>,
    rpc_url: String,
    view_ctx: Option<LocalViewCtx>,
) {
    let interval = shard_poll_interval();
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "shard loop: failed to build HTTP client; sharding disabled");
            return;
        }
    };
    // Advertise the held weight-shard slice on the work poll (if configured), so
    // the router's layer-affinity planner assigns this worker only segments whose
    // layer span it covers. A full-model worker (no layer range) sends just its
    // operator id — exactly today's behavior.
    let mut work_url = format!(
        "{}/shard/work?operator={}",
        state.router_base, state.operator_id
    );
    if let Some((lo, hi)) = state.layer_range {
        use std::fmt::Write as _;
        let _ = write!(
            work_url,
            "&layer_lo={lo}&layer_hi={hi}&has_embedding={}&has_classifier={}",
            state.has_embedding, state.has_classifier
        );
    }
    let result_url = format!("{}/shard/result", state.router_base);
    // Concurrent-job budget per node (`GK_SHARD_NODE_CONCURRENCY`, default 3):
    // sized so a 3-operator fleet can keep a 4-stage k=2 wavefront saturated
    // (4 stages x 2 executions / 3 operators < 3) without oversubscribing the
    // node's CPUs — each in-flight job is one single-threaded revm pass.
    let concurrency: usize = std::env::var("GK_SHARD_NODE_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(3);
    let job_slots = Arc::new(tokio::sync::Semaphore::new(concurrency));
    info!(
        operator = state.operator_id,
        work_url = %work_url,
        layer_range = ?state.layer_range,
        poll_ms = interval.as_millis() as u64,
        executor = if view_ctx.is_some() { "local" } else { "rpc" },
        overlay_models = view_ctx.as_ref().map(|c| c.overlay_files.len()).unwrap_or(0),
        concurrency,
        "shard loop started"
    );

    loop {
        let work = match client.get(&work_url).send().await {
            Ok(resp) if resp.status() == reqwest::StatusCode::NO_CONTENT => None,
            Ok(resp) => match resp.error_for_status() {
                Ok(ok) => ok.json::<ShardWork>().await.ok(),
                Err(e) => {
                    debug!(error = %e, "shard poll failed; retrying");
                    None
                }
            },
            Err(e) => {
                debug!(error = %e, "shard poll failed; retrying");
                None
            }
        };

        if let Some(work) = work {
            for job in work.jobs {
                // Execute jobs CONCURRENTLY (bounded by the semaphore): the
                // router's pipeline wavefront assigns one operator segments
                // from several stages at once, and a committee member must
                // not serialize behind a long segment it shares an operator
                // with. The heavy compute already rides the blocking pool
                // (local_view_call), so a spawned task per job costs one
                // permit, not a runtime worker.
                let permit = match Arc::clone(&job_slots).acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => return, // semaphore closed: shutting down
                };
                let client = client.clone();
                let rpc_url = rpc_url.clone();
                let view_ctx = view_ctx.clone();
                let state = Arc::clone(&state);
                let result_url = result_url.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let started = std::time::Instant::now();
                    let executed = match &view_ctx {
                        Some(ctx) => local_view_call(&client, &rpc_url, ctx, &job).await,
                        None => eth_call(&client, &rpc_url, &job).await,
                    };
                    match executed {
                        Ok(returndata) => {
                            let rd_hash = keccak256(&returndata);
                            match seg_chk(job.kind, &job.data, &returndata) {
                                Ok(chk) => {
                                    state.record(
                                        &job.infer_id,
                                        job.seg_id,
                                        ExecutedSeg {
                                            returndata_hash: rd_hash,
                                            chk,
                                        },
                                    );
                                    info!(
                                        infer_id = %job.infer_id,
                                        seg_id = job.seg_id,
                                        kind = ?job.kind,
                                        elapsed_ms = started.elapsed().as_millis() as u64,
                                        chk = %chk,
                                        "shard: executed segment"
                                    );
                                }
                                Err(e) => {
                                    warn!(seg_id = job.seg_id, error = %e, "shard: bad returndata shape");
                                    return;
                                }
                            }
                            let msg = ShardResultMsg {
                                infer_id: job.infer_id.clone(),
                                seg_id: job.seg_id,
                                operator: state.operator_id,
                                returndata,
                            };
                            if let Err(e) = client
                                .post(&result_url)
                                .json(&msg)
                                .send()
                                .await
                                .and_then(|r| r.error_for_status())
                            {
                                warn!(seg_id = job.seg_id, error = %e, "shard: failed to post result");
                            }
                        }
                        Err(e) => {
                            warn!(
                                infer_id = %job.infer_id,
                                seg_id = job.seg_id,
                                error = %e,
                                "shard: segment execution failed"
                            );
                        }
                    }
                });
            }
        }
        tokio::time::sleep(interval).await;
    }
}

/// Hex (de)serialization for byte payloads in the JSON channel.
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&format!("0x{}", hex::encode(bytes)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(de)?;
        hex::decode(s.trim_start_matches("0x")).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(seg_id: u64, chk_byte: u8, committee: Vec<u32>) -> ChainEntry {
        ChainEntry {
            seg_id,
            kind: SegKind::Forward,
            committee,
            chk: B256::from([chk_byte; 32]),
            returndata_hash: B256::from([chk_byte ^ 0xFF; 32]),
        }
    }

    fn chain(entries: Vec<ChainEntry>) -> ShardChain {
        let mut c = ShardChain {
            infer_id: "inf-1".into(),
            consumer: Address::from([9u8; 20]),
            prompt_ids: vec![7, 42, 99, 3],
            max_new: 6,
            answer_ids: vec![196, 73, 233],
            pipeline_root: B256::ZERO,
            entries,
        };
        c.pipeline_root = c.derive_root();
        c
    }

    #[test]
    fn derive_root_is_order_independent_of_entry_listing() {
        let a = chain(vec![entry(0, 1, vec![0, 1]), entry(1, 2, vec![1, 2])]);
        let b = chain(vec![entry(1, 2, vec![1, 2]), entry(0, 1, vec![0, 1])]);
        assert_eq!(a.derive_root(), b.derive_root());
    }

    #[test]
    fn gates_fires_on_fulfil_selector_respecting_allowlist() {
        let addr = Address::from([9u8; 20]);
        let other = Address::from([8u8; 20]);
        let mut fulfil = fulfilCall::SELECTOR.to_vec();
        fulfil.extend_from_slice(&[0u8; 32]);
        let not_fulfil = vec![0xAA, 0xBB, 0xCC, 0xDD, 0x00];

        // No allowlist: any fulfil call is gated, non-fulfil is not.
        let any = ShardState::new(0, None, "http://r".into());
        assert!(any.gates(addr, &fulfil));
        assert!(any.gates(other, &fulfil));
        assert!(!any.gates(addr, &not_fulfil));

        // Allowlist: only the configured consumer's fulfil calls.
        let scoped = ShardState::new(0, Some(addr), "http://r".into());
        assert!(scoped.gates(addr, &fulfil));
        assert!(!scoped.gates(other, &fulfil));
        assert!(!scoped.gates(addr, &not_fulfil));
    }

    #[test]
    fn forward_chk_is_third_head_word() {
        // ABI head for (bytes, bytes, bytes32): [off_x][off_kv][chk]...
        let mut rd = vec![0u8; 192];
        rd[95] = 0xAB;
        let chk = seg_chk(SegKind::Forward, &[], &rd).unwrap();
        assert_eq!(chk.as_slice()[31], 0xAB);
    }

    #[test]
    fn argmax_chk_binds_job_and_result() {
        let a = seg_chk(SegKind::Argmax, b"job1", b"res1").unwrap();
        let b = seg_chk(SegKind::Argmax, b"job2", b"res1").unwrap();
        let c = seg_chk(SegKind::Argmax, b"job1", b"res2").unwrap();
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn job_json_round_trips() {
        let job = ShardJob {
            infer_id: "inf-1".into(),
            seg_id: 3,
            kind: SegKind::Forward,
            to: Address::from([1u8; 20]),
            data: vec![0xDE, 0xAD],
            gas: 1 << 40,
        };
        let json = serde_json::to_string(&job).unwrap();
        let back: ShardJob = serde_json::from_str(&json).unwrap();
        assert_eq!(back.seg_id, 3);
        assert_eq!(back.data, vec![0xDE, 0xAD]);
        assert!(json.contains("0xdead"));
    }
}
