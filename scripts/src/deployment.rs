//! Resolving the deployed Gas Killer target out of the AVS deployment JSON.
//!
//! `deploy_example` records whichever example it deployed under a well-known `addresses` key (the
//! manifest's `alias`), so the tools that drive a target — `send_request`,
//! `verify_message_hash_parity`, `run_scenario`'s bare `local` sentinel, and `run_e2e_test.sh` —
//! resolve it without knowing which example is in play. Keeping the key here means it is named
//! once rather than spelled out at each of those call sites.

/// The `addresses` key the deployed Gas Killer target is recorded under.
pub const TARGET_ADDRESS_KEY: &str = "gasKillerTarget";

/// The key deployments used before the target key was named for its role rather than for the
/// first example that filled it. Read as a fallback so a deployment JSON produced by an older
/// `deploy_example` still resolves; droppable once every live deployment has been re-run.
pub const LEGACY_TARGET_ADDRESS_KEY: &str = "arraySummation";

/// Reads the target address out of a parsed deployment JSON, preferring
/// [`TARGET_ADDRESS_KEY`] and falling back to [`LEGACY_TARGET_ADDRESS_KEY`].
///
/// Returns `None` when neither key is present or the value is not a string; callers report the
/// missing-key case themselves, since the remedy differs per tool.
pub fn target_address(deployment: &serde_json::Value) -> Option<&str> {
    let addresses = deployment.get("addresses")?;
    addresses
        .get(TARGET_ADDRESS_KEY)
        .or_else(|| addresses.get(LEGACY_TARGET_ADDRESS_KEY))
        .and_then(|v| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_target_key() {
        let deployment = json!({"addresses": {TARGET_ADDRESS_KEY: "0x1234"}});
        assert_eq!(target_address(&deployment), Some("0x1234"));
    }

    #[test]
    fn falls_back_to_the_legacy_key() {
        let deployment = json!({"addresses": {LEGACY_TARGET_ADDRESS_KEY: "0xabcd"}});
        assert_eq!(target_address(&deployment), Some("0xabcd"));
    }

    /// A deployment carrying both — one written by the current `deploy_example` over an older
    /// file — must resolve the current key, not whichever happens to be found first.
    #[test]
    fn prefers_the_target_key_over_the_legacy_one() {
        let deployment = json!({
            "addresses": {
                LEGACY_TARGET_ADDRESS_KEY: "0xstale",
                TARGET_ADDRESS_KEY: "0xcurrent",
            }
        });
        assert_eq!(target_address(&deployment), Some("0xcurrent"));
    }

    #[test]
    fn returns_none_when_the_target_is_absent() {
        assert_eq!(target_address(&json!({"addresses": {}})), None);
        assert_eq!(target_address(&json!({})), None);
        // A non-string value is as unusable as a missing one.
        assert_eq!(
            target_address(&json!({"addresses": {TARGET_ADDRESS_KEY: 7}})),
            None
        );
    }
}
