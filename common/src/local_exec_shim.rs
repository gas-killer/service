//! Executor selection (`GK_SIM_EXECUTOR=rpc|local`) and overlay-source
//! preference (`GK_OVERLAY_MMAP=true|false`) for tracked-function analysis.
//!
//! gas-analyzer's local execution entry point
//! (`call_to_encoded_state_updates_local`, gas-analyzer#169) is pinned by
//! the workspace root `Cargo.toml` (branch `ron/local-execution`, a
//! superset of `ron/unbounded-v2-code-overlays` — it is stacked on top of
//! the UnboundedV1Xl and UNBOUNDED_V2 overlay commits). `gas_analyzer`
//! already re-exports the real [`gas_analyzer::SimExecutor`] selector
//! (`Rpc`/`Local`, with `parse`/`FromStr`/`Display`) and
//! [`gas_analyzer::LocalStateCache`]; this module only adds the
//! `GK_SIM_EXECUTOR`/`GK_OVERLAY_MMAP` env parsing, following the same
//! fail-loud pattern as `sim_profile_from_env` in `validator.rs`.
//!
//! ## The one real gap: mmap overlay mounting isn't public yet
//!
//! `call_to_encoded_state_updates_local`'s `overlay` parameter is
//! `Option<&OverlayEnv>` — the same in-RAM type the RPC path takes.
//! Internally, `LocalStateCache::overlay_mount_for` and
//! `OverlayMount::from_env` are the only things reachable from that public
//! signature; both require the blob already materialized as an `OverlayEnv`
//! (i.e. in RAM), even though `OverlayMount::from_files` — the mmap-backed
//! constructor that is the whole point of `GK_OVERLAY_MMAP` for 35GB models
//! — is `pub` on `gas_analyzer::OverlayMount` and used internally by
//! `LocalStateCache`. There is currently no public function that turns an
//! `OverlayMount::from_files` result into what `call_to_encoded_state_updates_local`
//! accepts (confirmed by reading `crates/evmsketch/src/lib.rs` +
//! `overlay_mount.rs` on `ron/local-execution` @ 81c642e: `overlay_mount_for`
//! is `pub(crate)`, and no `_local_files`-style entry point exists).
//!
//! So `GK_OVERLAY_MMAP` is parsed and threaded here (env surface + helm
//! surface are real and in place now), but [`prefer_mmap_overlay`] can only
//! be *read* by the validator today; actually avoiding the in-RAM
//! `OverlayEnv::from_blobs` read for the weights file requires one more
//! public entry point on the analyzer side. See the `TODO(gas-analyzer#169)`
//! on [`prefer_mmap_overlay`] for the exact one-line change once that
//! lands.

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
/// TODO(gas-analyzer#169): `call_to_encoded_state_updates_local`'s `overlay`
/// parameter is currently `Option<&OverlayEnv>` — there is no public path
/// from `OverlayMount::from_files` into it yet (see module docs). Once the
/// analyzer adds one (e.g. an `overlay_mount: Option<&OverlayMount>`
/// parameter, or an `OverlaySource` enum wrapping both), swap
/// `overlay_env_from_env`'s `OverlayEnv::from_blobs(&weights, &tokenizer)`
/// call in `validator.rs` for `OverlayMount::from_files(weights_path,
/// tokenizer_path, expected_manifest)` whenever
/// `prefer_mmap_overlay(sim_executor) == true`, and pass the mount through
/// the new parameter instead of `self.overlay_env.as_deref()`.
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
