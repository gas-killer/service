//! Resolving the deployed Gas Killer target out of the AVS deployment JSON.
//!
//! `deploy_example` records whichever example it deployed under a well-known `addresses` key (the
//! manifest's `alias`), so the tools that drive a target — `send_request`,
//! `verify_message_hash_parity`, `run_scenario`'s bare `local` sentinel, and `run_e2e_test.sh` —
//! resolve it without knowing which example is in play. Keeping the key here means it is named
//! once rather than spelled out at each of those call sites.

/// The `addresses` key the deployed Gas Killer target is recorded under.
pub const TARGET_ADDRESS_KEY: &str = "gasKillerTarget";

/// Reads the target address out of a parsed deployment JSON.
///
/// Returns `None` when the key is absent or its value is not a string; callers report that
/// themselves, since the remedy differs per tool. A deployment JSON predating this key resolves
/// by re-running `deploy_example`, or is bypassed with `GAS_KILLER_TARGET_ADDRESS`.
pub fn target_address(deployment: &serde_json::Value) -> Option<&str> {
    deployment
        .get("addresses")?
        .get(TARGET_ADDRESS_KEY)
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

    /// Only this key resolves. A deployment JSON carrying some other example's address under its
    /// own name is not a target — reading one would drive a contract the caller never selected.
    #[test]
    fn ignores_other_address_keys() {
        let deployment = json!({"addresses": {"arraySummation": "0xabcd"}});
        assert_eq!(target_address(&deployment), None);
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
