//! Executor selection (`GK_SIM_EXECUTOR=rpc|local`) and overlay-source
//! preference (`GK_OVERLAY_MMAP=true|false`) for tracked-function analysis.
//!
//! gas-analyzer's local execution entry points
//! (`call_to_encoded_state_updates_local` and its file-backed twins
//! `_local_files`/`_local_multi`, gas-analyzer#169/#172) are pinned by the
//! workspace root `Cargo.toml` (branch `ron/local-execution`, a superset of
//! `ron/unbounded-v2-code-overlays` — it is stacked on top of the
//! UnboundedV1Xl and UNBOUNDED_V2 overlay commits). `gas_analyzer` already
//! re-exports the real [`gas_analyzer::SimExecutor`] selector
//! (`Rpc`/`Local`, with `parse`/`FromStr`/`Display`) and
//! [`gas_analyzer::LocalStateCache`]; this module only adds the
//! `GK_SIM_EXECUTOR`/`GK_OVERLAY_MMAP` env parsing, following the same
//! fail-loud pattern as `sim_profile_from_env` in `validator.rs`.
//!
//! Under `GK_SIM_EXECUTOR=local` with mmap mode (the default), the
//! validator keeps only artifact file paths + pinned manifests and the
//! analyzer mounts them via `OverlayMount::from_files` — streaming-keccak
//! manifest verification, lazy chunk materialization, never the full blob
//! in heap. Multiple models (`GK_OVERLAY_WEIGHTS_2/...`, see
//! `validator::overlay_files_from_env`) mount simultaneously through
//! `call_to_encoded_state_updates_local_multi`: chunk addresses are derived
//! per-manifest, so distinct models' address sets are disjoint and one
//! composite lookup serves them all.

use anyhow::{Result, anyhow};

pub use gas_analyzer::SimExecutor;

/// Parses `GK_SIM_EXECUTOR` into a [`SimExecutor`]. Accepted values: `rpc`
/// (default) and `local` (case-insensitive) — delegates to
/// [`SimExecutor::parse`] for the actual matching so the accepted set can
/// never drift from the analyzer's own definition. Panics on any other
/// value: same fail-loud pattern as `sim_profile_from_env` — a typo'd
/// `GK_SIM_EXECUTOR` on one node while others run correctly wouldn't fork
/// the quorum (local and RPC execution byte-agree, see gas-analyzer#169's
/// differential tests) but would silently defeat the reason the operator
/// set `local` in the first place — e.g. no in-process RPC access to a
/// 35GB overlay artifact.
pub fn sim_executor_from_env() -> SimExecutor {
    match std::env::var("GK_SIM_EXECUTOR") {
        Err(_) => SimExecutor::Rpc,
        Ok(raw) if raw.trim().is_empty() => SimExecutor::Rpc,
        Ok(raw) => parse_sim_executor(&raw)
            .unwrap_or_else(|e| panic!("invalid GK_SIM_EXECUTOR {raw:?}: {e}")),
    }
}

fn parse_sim_executor(raw: &str) -> Result<SimExecutor> {
    SimExecutor::parse(raw).map_err(|e| anyhow!("{e}"))
}

/// Whether the local executor should prefer the mmap-backed
/// `OverlayMount::from_files` source over the in-RAM
/// `OverlayEnv::from_blobs` mount for 35GB-class artifacts.
///
/// Read from `GK_OVERLAY_MMAP` (`true`/`false`), defaulting to `true` when
/// `executor == SimExecutor::Local` (large artifacts must never be fully
/// materialized in RAM) and `false` under `SimExecutor::Rpc` (irrelevant:
/// the RPC path always overlays via `OverlayEnv`/`stateOverrides` JSON
/// regardless of this flag). Same fail-loud parsing as the other pinned-env
/// knobs.
///
/// When this returns `true`, `validator::overlay_files_from_env` keeps only
/// artifact paths + manifests and analysis goes through the analyzer's
/// file-backed entry point (`call_to_encoded_state_updates_local_multi`) —
/// this is also the only mode that supports mounting MULTIPLE models
/// (`GK_OVERLAY_WEIGHTS_2/...`); the in-RAM `OverlayEnv` path serves one
/// model only and refuses indexed overlay slots at startup.
pub fn prefer_mmap_overlay(executor: SimExecutor) -> bool {
    match std::env::var("GK_OVERLAY_MMAP") {
        Err(_) => executor == SimExecutor::Local,
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "" => executor == SimExecutor::Local,
            "true" | "1" => true,
            "false" | "0" => false,
            other => panic!("invalid GK_OVERLAY_MMAP {other:?}: expected \"true\" or \"false\""),
        },
    }
}
