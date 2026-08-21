//! Decoding of the revert that `eth_estimateGas` reports for a rendered `verifyAndUpdate`.
//!
//! The router estimates the apply cost of every payload it renders. That call runs the exact
//! calldata the client would submit, against the target's current state and through the same
//! signature verification, so a revert there is proof the payload cannot land. This module turns
//! the revert data into something the client can act on: the Solidity error name plus the
//! condition it implies about the target's configuration or the round's inputs.
//!
//! A reverting `verifyAndUpdate` can originate in two places. The SDK's own preflight checks and
//! state-change handler are taken from the generated bindings, so their selectors and signatures
//! track the committed ABIs and a rename or a changed argument list upstream is a compile error
//! here rather than an entry that silently stops matching. The EigenLayer `BLSSignatureChecker`
//! the target calls into is the exception: its errors do not appear in the target's ABI — the
//! target only calls the checker, so solc does not surface them — so those are declared locally.

use alloy::rpc::json_rpc::ErrorPayload;
use alloy::sol_types::{GenericContractError, SolError, SolInterface};
use alloy_primitives::{Bytes, FixedBytes, hex};
use gas_killer_common::bindings::gaskillersdk::GasKillerSDK;
use gas_killer_common::bindings::schnorrgaskillersdk::SchnorrGasKillerSDK;
use std::fmt;

// EigenLayer's `IBLSSignatureCheckerErrors`, raised inside the `checkSignatures` call the target
// makes. Declared here because no ABI committed to this repo carries them, and only so the
// selector table can take each selector and signature from a compile-time constant rather than a
// transcribed hex literal; no instance of these types is ever built.
mod checker_errors {
    alloy::sol! {
        error InputEmptyQuorumNumbers();
        error InputArrayLengthMismatch();
        error InputNonSignerLengthMismatch();
        error InvalidReferenceBlocknumber();
        error NonSignerPubkeysNotSorted();
        error InvalidQuorumApkHash();
        error InvalidBLSPairingKey();
        error InvalidBLSSignature();
    }
}

/// JSON-RPC error code a node returns for a call that executed and reverted. Every standard
/// client uses it, so it is the primary signal.
const EXECUTION_REVERTED_CODE: i64 = 3;

/// Message fragments that mean the node executed the call and the call reverted, matched
/// case-insensitively and consulted only when the response code is not
/// [`EXECUTION_REVERTED_CODE`].
///
/// Whole phrases rather than the bare word "revert": a rejection raised *before* execution must
/// keep the advisory fallback estimate, and these phrases carry only the one meaning in every
/// client that emits them. A client whose wording matches neither the code nor a phrase falls
/// through to the fallback, which is the safe direction — the payload ships as it did before this
/// check existed, rather than a completed round being failed on a guess.
const EXECUTION_REVERT_MESSAGES: &[&str] = &["execution reverted", "reverted with", "vm exception"];

/// Longest raw revert payload rendered in full. Beyond this the hex is truncated so an error
/// carrying large `bytes` arguments cannot flood a log line or a client-visible task error.
const MAX_RENDERED_REVERT_BYTES: usize = 32;

/// A revert selector the router recognises, paired with the condition that raises it.
struct KnownRevert {
    selector: FixedBytes<4>,
    /// Solidity error signature, e.g. `InvalidQuorumApkHash()`.
    signature: &'static str,
    /// One line on what the revert means for whoever has to act on it.
    cause: &'static str,
}

/// Builds a [`KnownRevert`] from a declared Solidity error, taking its selector and signature from
/// the `sol!`-generated constants so neither is transcribed by hand.
macro_rules! known_revert {
    ($err:ty, $cause:literal) => {
        KnownRevert {
            selector: FixedBytes::new(<$err as SolError>::SELECTOR),
            signature: <$err as SolError>::SIGNATURE,
            cause: $cause,
        }
    };
}

/// Selectors the router can explain, searched linearly: the table is short and only consulted on
/// the failure path.
const KNOWN_REVERTS: &[KnownRevert] = &[
    known_revert!(
        checker_errors::InvalidQuorumApkHash,
        "the target's blsSignatureChecker resolves a different operator set than the one that \
         signed this task; check the target's avsAddress and blsSignatureChecker against the live \
         deployment"
    ),
    known_revert!(
        checker_errors::InvalidBLSSignature,
        "the aggregate signature does not verify against the quorum aggregate public key at the \
         reference block"
    ),
    known_revert!(
        checker_errors::InvalidBLSPairingKey,
        "the BN254 pairing precompile rejected the proof; a supplied public key is not a valid \
         curve point"
    ),
    known_revert!(
        checker_errors::InvalidReferenceBlocknumber,
        "the reference block is outside the window the signature checker accepts"
    ),
    known_revert!(
        checker_errors::InputEmptyQuorumNumbers,
        "the round carried no quorum numbers for the signature checker to verify against"
    ),
    known_revert!(
        checker_errors::InputArrayLengthMismatch,
        "the non-signer proof assembled for this round has inconsistent array lengths"
    ),
    known_revert!(
        checker_errors::InputNonSignerLengthMismatch,
        "the non-signer public keys and their quorum bitmap indices differ in length"
    ),
    known_revert!(
        checker_errors::NonSignerPubkeysNotSorted,
        "the non-signer public keys are not in the ascending order the signature checker requires"
    ),
    known_revert!(
        GasKillerSDK::FutureBlockNumber,
        "the reference block is not yet mined from the target's view; the target may be on a \
         different chain than the one this task was analysed against"
    ),
    known_revert!(
        GasKillerSDK::StaleBlockNumber,
        "the reference block is older than the target's blockStaleMeasure allows; request a fresh \
         payload"
    ),
    known_revert!(
        GasKillerSDK::InvalidTransitionIndex,
        "the target's stateTransitionCount has moved past this payload; request a fresh payload"
    ),
    known_revert!(
        GasKillerSDK::InvalidSignature,
        "the signed digest does not match the one the target recomputes from its own address, the \
         target function, and the storage updates"
    ),
    known_revert!(
        GasKillerSDK::InsufficientQuorumThreshold,
        "the operators that signed hold less than QUORUM_THRESHOLD of the quorum's stake"
    ),
    known_revert!(
        GasKillerSDK::InvalidStorageUpdates,
        "the target's state-change handler could not decode the encoded storage updates"
    ),
    known_revert!(
        GasKillerSDK::MalformedLogPayload,
        "an encoded LOG update is not shaped as the target's state-change handler expects"
    ),
    known_revert!(
        GasKillerSDK::InvalidOperation,
        "the storage updates contain an operation the target's state-change handler does not \
         implement"
    ),
    known_revert!(
        GasKillerSDK::InvalidArguments,
        "the target rejected the call arguments"
    ),
    known_revert!(
        GasKillerSDK::RevertingContext,
        "a CALL replayed from the storage updates reverted inside the target"
    ),
    known_revert!(
        GasKillerSDK::DeploymentFailed,
        "a CREATE or CREATE2 replayed from the storage updates failed"
    ),
    known_revert!(
        SchnorrGasKillerSDK::InvalidQuorumSignature,
        "the aggregate Schnorr signature does not verify against the registry's aggregate key at \
         the reference block"
    ),
    known_revert!(
        SchnorrGasKillerSDK::ReentrantTransition,
        "the target is already inside a tracked transition, so verifyAndUpdate cannot be entered"
    ),
    known_revert!(
        SchnorrGasKillerSDK::EmptyBatch,
        "the round produced no state updates to apply"
    ),
    known_revert!(
        SchnorrGasKillerSDK::BlockStaleMeasureOverflow,
        "the target's configured blockStaleMeasure overflows when added to the reference block"
    ),
];

/// The revert behind a `verifyAndUpdate` that cannot execute, rendered for an operator log line
/// and the client-visible task error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadRevert(Bytes);

impl PayloadRevert {
    /// Extracts the revert behind a failed contract call, or `None` when the call never reached
    /// execution.
    ///
    /// The distinction is what makes the estimate usable as a landability check: a transport
    /// failure, timeout, or rate-limit rejection says nothing about whether the call would have
    /// succeeded, so only an error the node produced by executing the call qualifies. Reverts that
    /// carry no return data are still reverts, and are recognised from the node's error response
    /// rather than from data it did not send.
    pub fn from_call_error(err: &alloy::contract::Error) -> Option<Self> {
        if let Some(data) = err.as_revert_data() {
            return Some(Self(data));
        }

        let alloy::contract::Error::TransportError(transport) = err else {
            return None;
        };
        let response = transport.as_error_resp()?;
        if !reports_execution_revert(response) {
            return None;
        }
        // `as_revert_data` reads the payload only when the node's message contains a lowercase
        // "revert", so a client that capitalises its wording arrives here with revert data still
        // attached. Reading it again is what keeps the selector — the entire diagnostic value of
        // this path — from being dropped on precisely the non-standard clients the message check
        // exists to serve.
        Some(Self(revert_data(response).unwrap_or_default()))
    }

    /// Raw revert data as the node returned it. Empty when the call reverted without a reason.
    pub fn data(&self) -> &Bytes {
        &self.0
    }

    /// The table entry for this revert's selector, if the router knows it.
    fn known(&self) -> Option<&'static KnownRevert> {
        let selector: [u8; 4] = self.0.get(..4)?.try_into().ok()?;
        KNOWN_REVERTS
            .iter()
            .find(|known| known.selector.0 == selector)
    }
}

impl fmt::Display for PayloadRevert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("reverted without reason data");
        }
        if let Some(known) = self.known() {
            return write!(
                f,
                "{} ({}): {}",
                known.signature,
                hex::encode_prefixed(&self.0[..4]),
                known.cause
            );
        }
        // `require(cond, "reason")` and compiler-inserted panics are the two standard-encoded
        // reverts; the raw data is all there is to report for anything else.
        if let Ok(standard) = GenericContractError::abi_decode(&self.0) {
            return write!(f, "{standard}");
        }
        if self.0.len() > MAX_RENDERED_REVERT_BYTES {
            return write!(
                f,
                "unrecognised revert data {}… ({} bytes)",
                hex::encode_prefixed(&self.0[..MAX_RENDERED_REVERT_BYTES]),
                self.0.len()
            );
        }
        write!(
            f,
            "unrecognised revert data {}",
            hex::encode_prefixed(&self.0)
        )
    }
}

/// Whether a JSON-RPC error response describes a call that executed and reverted, as opposed to
/// one the node refused before executing.
fn reports_execution_revert(response: &ErrorPayload) -> bool {
    if response.code == EXECUTION_REVERTED_CODE {
        return true;
    }
    let message = response.message.to_lowercase();
    EXECUTION_REVERT_MESSAGES
        .iter()
        .any(|phrase| message.contains(phrase))
}

/// Pulls the revert payload out of a node's error `data`, which clients send either as a hex
/// string or wrapped in an object nesting the real payload under a further `data`.
///
/// Keyed on the field name rather than taking the first string in the structure that parses as
/// hex, because sibling fields are not reliably unparseable: an empty string decodes to empty
/// bytes and a stringified number is valid hex, so a first-match scan can report a neighbouring
/// field as the payload, or stop on one and drop the payload entirely. Only a non-empty decode
/// counts for the same reason — an empty one carries no more information than finding nothing, and
/// the caller already renders a missing payload as a revert without a reason.
fn revert_data(response: &ErrorPayload) -> Option<Bytes> {
    fn decoded(encoded: &str) -> Option<Bytes> {
        encoded
            .parse::<Bytes>()
            .ok()
            .filter(|data| !data.is_empty())
    }

    /// Follows `data` down through however many objects a client wraps the payload in.
    fn keyed(value: &serde_json::Value) -> Option<Bytes> {
        match value {
            serde_json::Value::String(encoded) => decoded(encoded),
            serde_json::Value::Object(fields) => keyed(fields.get("data")?),
            _ => None,
        }
    }

    /// Last resort for a client that names the field something else: any hex string anywhere in
    /// the structure. Only reached once the keyed lookup has found nothing.
    fn scan(value: &serde_json::Value) -> Option<Bytes> {
        match value {
            serde_json::Value::String(encoded) => decoded(encoded),
            serde_json::Value::Object(fields) => fields.values().find_map(scan),
            _ => None,
        }
    }

    let data = response.try_data_as::<serde_json::Value>()?.ok()?;
    keyed(&data).or_else(|| scan(&data))
}

/// Builds the contract error a node returns for a call that executed and reverted, with `data` as
/// the hex revert payload (`None` for a bare `revert()`).
///
/// Test-only: in production these only ever arrive from a provider, and both this module and the
/// executor need to drive the estimation path without a chain.
#[cfg(test)]
pub(crate) fn execution_reverted(data: Option<&str>) -> alloy::contract::Error {
    rpc_error_with_data(
        EXECUTION_REVERTED_CODE,
        "execution reverted",
        data.map(|d| format!("\"{d}\"")),
    )
}

/// Builds a JSON-RPC error response with no revert payload, for the failures that never reach
/// execution. Test-only, as with [`execution_reverted`].
#[cfg(test)]
pub(crate) fn rpc_error(code: i64, message: &'static str) -> alloy::contract::Error {
    rpc_error_with_data(code, message, None)
}

/// `data` is raw JSON, so a test can queue the bare hex string most clients send or the object
/// some of them nest it in. Test-only, as with [`execution_reverted`].
#[cfg(test)]
fn rpc_error_with_data(
    code: i64,
    message: &'static str,
    data: Option<String>,
) -> alloy::contract::Error {
    use alloy::transports::RpcError;
    use serde_json::value::RawValue;

    alloy::contract::Error::TransportError(RpcError::ErrorResp(ErrorPayload {
        code,
        message: message.into(),
        data: data.map(|d| RawValue::from_string(d).unwrap()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::Revert;
    use alloy::transports::TransportErrorKind;

    fn revert(data: &str) -> PayloadRevert {
        PayloadRevert(Bytes::from(
            hex::decode(data).expect("test revert data should be hex"),
        ))
    }

    // Every error the committed SDK ABIs declare must have a cause. The generated bindings make a
    // renamed, removed, or retyped error a compile failure, but an error *added* upstream would
    // otherwise pass unnoticed and reach clients as raw revert data — which is the outcome this
    // module exists to avoid. Reading the ABIs directly is the only check that sees additions.
    #[test]
    fn every_error_the_sdks_declare_has_a_cause() {
        for (contract, abi) in [
            (
                "GasKillerSDK",
                gas_killer_common::bindings::GAS_KILLER_SDK_ABI,
            ),
            (
                "SchnorrGasKillerSDK",
                gas_killer_common::bindings::SCHNORR_GAS_KILLER_SDK_ABI,
            ),
        ] {
            let declared = serde_json::from_str::<alloy_json_abi::ContractObject>(abi)
                .expect("the committed ABI should parse")
                .abi
                .expect("the committed ABI should carry an abi section");

            for error in declared.errors.values().flatten() {
                let selector = FixedBytes::new(error.selector().0);
                assert!(
                    KNOWN_REVERTS.iter().any(|known| known.selector == selector),
                    "{contract}.{} ({selector}) is declared in the committed ABI but has no cause \
                     in KNOWN_REVERTS; add an entry so a client sees the reason instead of raw \
                     revert data",
                    error.signature(),
                );
            }
        }
    }

    // A second table entry for an already-listed selector would be dead: the lookup takes the
    // first match, so a duplicate silently shadows whichever cause was written second.
    #[test]
    fn every_known_selector_appears_once() {
        for (i, entry) in KNOWN_REVERTS.iter().enumerate() {
            let duplicate = KNOWN_REVERTS
                .iter()
                .skip(i + 1)
                .find(|other| other.selector == entry.selector);
            assert!(
                duplicate.is_none(),
                "{} shares a selector with {}",
                entry.signature,
                duplicate.map(|d| d.signature).unwrap_or_default()
            );
        }
    }

    // The selector the mis-wired integrator target produced. It must name the error and point at
    // the configuration that causes it, since the selector alone told them nothing.
    #[test]
    fn misconfigured_signature_checker_is_named_and_explained() {
        let rendered = revert("0xe1310aed").to_string();
        assert!(rendered.contains("InvalidQuorumApkHash()"), "{rendered}");
        assert!(rendered.contains("0xe1310aed"), "{rendered}");
        assert!(rendered.contains("blsSignatureChecker"), "{rendered}");
    }

    // Selectors are derived from the `sol!` declarations, so this pins the derivation itself
    // against an independently computed value rather than re-asserting the table.
    #[test]
    fn table_selectors_match_the_solidity_signatures() {
        let entry = revert("0xe1310aed")
            .known()
            .expect("InvalidQuorumApkHash should be a known selector");
        assert_eq!(entry.signature, "InvalidQuorumApkHash()");
        assert_eq!(entry.selector, FixedBytes::new([0xe1, 0x31, 0x0a, 0xed]));
    }

    #[test]
    fn sdk_preflight_selectors_are_recognised() {
        // StaleBlockNumber() and InvalidTransitionIndex(): the two reverts a payload that sat too
        // long, or was overtaken by another transition, produces.
        assert!(
            revert("0x305c3e93")
                .to_string()
                .contains("StaleBlockNumber()")
        );
        assert!(
            revert("0x7376e0a2")
                .to_string()
                .contains("InvalidTransitionIndex()")
        );
    }

    #[test]
    fn a_revert_without_reason_data_says_so() {
        assert_eq!(
            PayloadRevert(Bytes::new()).to_string(),
            "reverted without reason data"
        );
    }

    // A `require(cond, "reason")` in the target's own code is standard-encoded rather than a
    // custom error, so the string is worth surfacing verbatim.
    #[test]
    fn a_require_string_is_surfaced() {
        let encoded = Bytes::from(Revert::from("vault is paused").abi_encode());
        assert!(
            PayloadRevert(encoded)
                .to_string()
                .contains("vault is paused"),
            "the revert string should reach the message"
        );
    }

    #[test]
    fn an_unrecognised_selector_reports_its_raw_data() {
        assert_eq!(
            revert("0xdeadbeef").to_string(),
            "unrecognised revert data 0xdeadbeef"
        );
    }

    // Errors carrying `bytes` arguments can be arbitrarily large, and the rendering ends up in a
    // log line and a client-visible task error.
    #[test]
    fn oversized_unrecognised_data_is_truncated() {
        let mut data = vec![0xde, 0xad, 0xbe, 0xef];
        data.extend(std::iter::repeat_n(0xab, 200));
        let rendered = PayloadRevert(Bytes::from(data)).to_string();
        assert!(rendered.contains("(204 bytes)"), "{rendered}");
        assert!(
            rendered.len() < 120,
            "rendering should stay short: {rendered}"
        );
    }

    #[test]
    fn a_revert_response_carries_its_data_through() {
        let revert = PayloadRevert::from_call_error(&execution_reverted(Some("0xe1310aed")))
            .expect("an execution revert should be classified as a revert");
        assert_eq!(revert.data().as_ref(), [0xe1, 0x31, 0x0a, 0xed]);
    }

    // A bare `revert()` returns no data, but the call still executed and still failed, so the
    // payload is still unsubmittable.
    #[test]
    fn a_revert_response_without_data_is_still_a_revert() {
        let revert = PayloadRevert::from_call_error(&execution_reverted(None))
            .expect("a data-less execution revert should still be classified as a revert");
        assert!(revert.data().is_empty());
    }

    // Not every client uses code 3; a recognised phrase is the fallback signal.
    #[test]
    fn a_revert_reported_under_a_client_specific_code_is_recognised() {
        assert!(PayloadRevert::from_call_error(&rpc_error(-32000, "execution reverted")).is_some());
        assert!(
            PayloadRevert::from_call_error(&rpc_error(
                -32000,
                "VM Exception while processing transaction: reverted with reason string 'paused'"
            ))
            .is_some()
        );
    }

    // The phrase list exists so the bare word cannot carry a classification on its own: a
    // rejection that never executed must keep the fallback estimate, however it is worded.
    #[test]
    fn the_bare_word_revert_does_not_make_an_error_a_revert() {
        assert!(
            PayloadRevert::from_call_error(&rpc_error(
                -32000,
                "cannot revert to an archived state"
            ))
            .is_none()
        );
    }

    // `as_revert_data` only reads the payload when the message contains a lowercase "revert", so a
    // client that capitalises its wording would otherwise be reported as a data-less revert with
    // the selector sitting unread in the response.
    #[test]
    fn revert_data_survives_a_capitalised_message() {
        let revert = PayloadRevert::from_call_error(&rpc_error_with_data(
            EXECUTION_REVERTED_CODE,
            "Reverted",
            Some("\"0xe1310aed\"".to_string()),
        ))
        .expect("a capitalised revert message should still classify as a revert");
        assert_eq!(revert.data().as_ref(), [0xe1, 0x31, 0x0a, 0xed]);
        assert!(
            revert.to_string().contains("InvalidQuorumApkHash()"),
            "the recovered selector should still be named: {revert}"
        );
    }

    // Some clients wrap the payload in an object alongside the message rather than sending the hex
    // string directly.
    #[test]
    fn revert_data_is_recovered_from_a_nested_data_object() {
        let revert = PayloadRevert::from_call_error(&rpc_error_with_data(
            EXECUTION_REVERTED_CODE,
            "Reverted",
            Some(r#"{"message":"execution reverted","data":"0xe1310aed"}"#.to_string()),
        ))
        .expect("a nested revert payload should still classify as a revert");
        assert_eq!(revert.data().as_ref(), [0xe1, 0x31, 0x0a, 0xed]);
    }

    // Sibling fields are not reliably unparseable: a stringified number is valid hex, so a scan
    // that takes the first match can report a neighbour as the revert payload.
    #[test]
    fn revert_data_comes_from_the_data_field_not_a_sibling() {
        let revert = PayloadRevert::from_call_error(&rpc_error_with_data(
            EXECUTION_REVERTED_CODE,
            "Reverted",
            Some(r#"{"gasUsed":"1234","data":"0xe1310aed"}"#.to_string()),
        ))
        .expect("a keyed revert payload should classify as a revert");
        assert_eq!(revert.data().as_ref(), [0xe1, 0x31, 0x0a, 0xed]);
    }

    // An empty string decodes to empty bytes, so a scan would stop on one and report a revert with
    // no reason while the selector sat unread in the next field.
    #[test]
    fn an_empty_sibling_field_does_not_mask_the_revert_data() {
        let revert = PayloadRevert::from_call_error(&rpc_error_with_data(
            EXECUTION_REVERTED_CODE,
            "Reverted",
            Some(r#"{"cause":"","data":"0xe1310aed"}"#.to_string()),
        ))
        .expect("an empty sibling should not stop the lookup");
        assert_eq!(revert.data().as_ref(), [0xe1, 0x31, 0x0a, 0xed]);
        assert!(
            revert.to_string().contains("InvalidQuorumApkHash()"),
            "{revert}"
        );
    }

    // Clients that wrap the payload under some other field name still get read, once the keyed
    // lookup has come up empty.
    #[test]
    fn revert_data_is_found_under_an_unconventional_field_name() {
        let revert = PayloadRevert::from_call_error(&rpc_error_with_data(
            EXECUTION_REVERTED_CODE,
            "Reverted",
            Some(r#"{"originalError":{"revertData":"0xe1310aed"}}"#.to_string()),
        ))
        .expect("an unconventionally named payload should still be found");
        assert_eq!(revert.data().as_ref(), [0xe1, 0x31, 0x0a, 0xed]);
    }

    // A client that reports a revert without data must still be classified as a revert; there is
    // simply nothing to name.
    #[test]
    fn a_capitalised_message_without_data_stays_a_data_less_revert() {
        let revert = PayloadRevert::from_call_error(&rpc_error_with_data(
            EXECUTION_REVERTED_CODE,
            "Reverted",
            None,
        ))
        .expect("a data-less revert should still classify as a revert");
        assert!(revert.data().is_empty());
        assert_eq!(revert.to_string(), "reverted without reason data");
    }

    // Estimation refusing before it executes says nothing about whether the call would land, so
    // these must not be mistaken for reverts — the payload keeps its fallback estimate instead.
    #[test]
    fn errors_raised_before_execution_are_not_reverts() {
        assert!(
            PayloadRevert::from_call_error(&rpc_error(
                -32000,
                "gas required exceeds allowance (30000000)"
            ))
            .is_none()
        );
        assert!(
            PayloadRevert::from_call_error(&rpc_error(-32005, "rate limit exceeded")).is_none()
        );
    }

    #[test]
    fn a_transport_failure_is_not_a_revert() {
        let err = alloy::contract::Error::TransportError(TransportErrorKind::custom_str(
            "connection reset by peer",
        ));
        assert!(PayloadRevert::from_call_error(&err).is_none());
    }
}
