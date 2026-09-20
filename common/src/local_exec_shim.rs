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
//!
//! UNBOUNDED_V3 (gkvm, gas-analyzer#197): under `GK_SIM_EXECUTOR=local` the
//! analyzer also serves the guest-VM precompile. The operator installs guest
//! programs and artifacts through the analyzer's own `GK_GUEST_PROGRAM[_N]` +
//! `GK_GUEST_PROGRAM_HASH[_N]` / `GK_GUEST_ARTIFACT[_N]` +
//! `GK_GUEST_ARTIFACT_ROOT[_N]` slots; [`gkvm_host_from_env`] loads and
//! verifies them once at validator construction, and the validator hangs the
//! resulting [`GkvmHost`] on its `LocalStateCache`, which every local entry
//! point already receives — no new analyzer call. The guest VM does not
//! exist under `rpc`: there the precompile address is an empty account, the
//! consumer reverts `GkVmUnavailable`, and that revert is a signable
//! transition the `local` operators do not produce — so `GK_GUEST_*` under
//! `rpc` refuses to start.

use std::sync::Arc;

use anyhow::{Result, anyhow};

pub use gas_analyzer::SimExecutor;
// The gkvm config types, so the rest of the service names them from here.
pub use gas_analyzer::{
    ArtifactMountV3, GkVmMountError, GkvmHost, GkvmHostError, GuestProgramSet, LoadedGuestProgram,
};

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

/// Whether any `GK_GUEST_*` slot variable carries a value. Empty values are
/// "unset", as for the overlay slots (Helm renders unset values as `""`).
fn guest_vm_configured_from(mut vars: impl Iterator<Item = (String, String)>) -> bool {
    vars.any(|(key, value)| key.starts_with("GK_GUEST_") && !value.trim().is_empty())
}

/// Loads the operator's installed guest programs and artifacts
/// (`GK_GUEST_PROGRAM[_N]` / `GK_GUEST_ARTIFACT[_N]`, each with its committed
/// hash/root) into a [`GkvmHost`] for the local executor. `None` when no
/// `GK_GUEST_*` variable is set — the analyzer then fails any gkvm call as an
/// environment error, so this operator abstains on guest-VM tasks instead of
/// signing anything.
///
/// Every mounted artifact is served front to back
/// ([`GkvmHost::with_sequential_schedules`]): the env slots carry no schedule
/// and every shipped guest loads its artifacts once, sequentially. A guest
/// that reads in another order traps deterministically — on every operator
/// alike.
///
/// Panics, same fail-loud pattern as [`sim_executor_from_env`]: on a slot
/// that is half-configured, unreadable, or whose bytes do not hash to the
/// committed value (an operator running different bytes under the same
/// commitment would sign divergent results); and on `GK_GUEST_*` under
/// `GK_SIM_EXECUTOR=rpc`, where the guest VM does not exist and every
/// guest-VM task would become a signed `GkVmUnavailable` revert transition.
pub fn gkvm_host_from_env(executor: SimExecutor) -> Option<Arc<GkvmHost>> {
    if !guest_vm_configured_from(std::env::vars()) {
        return None;
    }
    assert!(
        executor == SimExecutor::Local,
        "GK_GUEST_* is configured but GK_SIM_EXECUTOR is {executor}: guest programs run only \
         under GK_SIM_EXECUTOR=local (the rpc executor would sign GkVmUnavailable reverts)"
    );
    let host = GkvmHost::from_env()
        .unwrap_or_else(|e| panic!("invalid GK_GUEST_* configuration: {e}"))
        .with_sequential_schedules();
    tracing::info!(
        programs = ?host.programs().program_hashes(),
        artifacts = ?host.programs().artifact_roots(),
        "UNBOUNDED_V3: guest programs installed for the gkvm precompile"
    );
    Some(Arc::new(host))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Iterator<Item = (String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn test_guest_vm_configured_ignores_empty_and_unrelated_vars() {
        assert!(!guest_vm_configured_from(vars(&[])));
        assert!(!guest_vm_configured_from(vars(&[
            ("GK_GUEST_PROGRAM", ""),
            ("GK_GUEST_PROGRAM_HASH", "  "),
            ("GK_SIM_EXECUTOR", "local"),
            ("GK_OVERLAY_WEIGHTS", "/m/weights.bin"),
        ])));
        assert!(guest_vm_configured_from(vars(&[(
            "GK_GUEST_PROGRAM",
            "/g/answer.elf"
        )])));
        // A lone digest still counts: the analyzer then refuses the
        // half-configured slot loudly instead of this operator silently
        // abstaining.
        assert!(guest_vm_configured_from(vars(&[(
            "GK_GUEST_ARTIFACT_ROOT_2",
            "0x00"
        )])));
    }
}
