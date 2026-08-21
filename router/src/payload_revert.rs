//! Decoding of the revert that `eth_estimateGas` reports for a rendered `verifyAndUpdate`.
//!
//! The router estimates the apply cost of every payload it renders. That call runs the exact
//! calldata the client would submit, against the target's current state and through the same
//! signature verification, so a revert there is proof the payload cannot land. This module turns
//! the revert data into something the client can act on: the Solidity error name plus the
//! condition it implies about the target's configuration or the round's inputs.
//!
//! A reverting `verifyAndUpdate` can originate in three places, so selectors are collected from
//! all of them: the SDK's own preflight checks, the EigenLayer `BLSSignatureChecker` the target
//! calls into, and the Schnorr SDK's equivalents. The middleware errors do not appear in the
//! target's ABI — the target only calls the checker, so solc does not surface them — which is why
//! they are declared here rather than read from the generated bindings.

use alloy::sol_types::{GenericContractError, SolError, SolInterface};
use alloy_primitives::{Bytes, FixedBytes, hex};
use std::fmt;

// Declared only so the selector table can take each selector and signature from a compile-time
// constant rather than a transcribed hex literal; no instance of these types is ever built.
mod sol_errors {
    alloy::sol! {
        // EigenLayer `IBLSSignatureCheckerErrors`, raised inside `checkSignatures`.
        error InputEmptyQuorumNumbers();
        error InputArrayLengthMismatch();
        error InputNonSignerLengthMismatch();
        error InvalidReferenceBlocknumber();
        error NonSignerPubkeysNotSorted();
        error InvalidQuorumApkHash();
        error InvalidBLSPairingKey();
        error InvalidBLSSignature();

        // `GasKillerSDK` preflight checks and state-change handler. Shared with the Schnorr SDK
        // where the signature is identical, so each is declared once.
        error FutureBlockNumber();
        error StaleBlockNumber();
        error InvalidTransitionIndex();
        error InvalidSignature();
        error InsufficientQuorumThreshold();
        error InvalidStorageUpdates();
        error MalformedLogPayload();
        error InvalidOperation();
        error InvalidArguments();
        error RevertingContext(uint256 index, address target, bytes callData, bytes returnData);
        error DeploymentFailed();

        // `SchnorrGasKillerSDK`-only errors.
        error InvalidQuorumSignature();
        error ReentrantTransition();
        error EmptyBatch();
        error BlockStaleMeasureOverflow();
    }
}

/// JSON-RPC error code a node returns for a call that executed and reverted.
const EXECUTION_REVERTED_CODE: i64 = 3;

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
        sol_errors::InvalidQuorumApkHash,
        "the target's blsSignatureChecker resolves a different operator set than the one that \
         signed this task; check the target's avsAddress and blsSignatureChecker against the live \
         deployment"
    ),
    known_revert!(
        sol_errors::InvalidBLSSignature,
        "the aggregate signature does not verify against the quorum aggregate public key at the \
         reference block"
    ),
    known_revert!(
        sol_errors::InvalidBLSPairingKey,
        "the BN254 pairing precompile rejected the proof; a supplied public key is not a valid \
         curve point"
    ),
    known_revert!(
        sol_errors::InvalidReferenceBlocknumber,
        "the reference block is outside the window the signature checker accepts"
    ),
    known_revert!(
        sol_errors::InputEmptyQuorumNumbers,
        "the round carried no quorum numbers for the signature checker to verify against"
    ),
    known_revert!(
        sol_errors::InputArrayLengthMismatch,
        "the non-signer proof assembled for this round has inconsistent array lengths"
    ),
    known_revert!(
        sol_errors::InputNonSignerLengthMismatch,
        "the non-signer public keys and their quorum bitmap indices differ in length"
    ),
    known_revert!(
        sol_errors::NonSignerPubkeysNotSorted,
        "the non-signer public keys are not in the ascending order the signature checker requires"
    ),
    known_revert!(
        sol_errors::FutureBlockNumber,
        "the reference block is not yet mined from the target's view; the target may be on a \
         different chain than the one this task was analysed against"
    ),
    known_revert!(
        sol_errors::StaleBlockNumber,
        "the reference block is older than the target's blockStaleMeasure allows; request a fresh \
         payload"
    ),
    known_revert!(
        sol_errors::InvalidTransitionIndex,
        "the target's stateTransitionCount has moved past this payload; request a fresh payload"
    ),
    known_revert!(
        sol_errors::InvalidSignature,
        "the signed digest does not match the one the target recomputes from its own address, the \
         target function, and the storage updates"
    ),
    known_revert!(
        sol_errors::InsufficientQuorumThreshold,
        "the operators that signed hold less than QUORUM_THRESHOLD of the quorum's stake"
    ),
    known_revert!(
        sol_errors::InvalidStorageUpdates,
        "the target's state-change handler could not decode the encoded storage updates"
    ),
    known_revert!(
        sol_errors::MalformedLogPayload,
        "an encoded LOG update is not shaped as the target's state-change handler expects"
    ),
    known_revert!(
        sol_errors::InvalidOperation,
        "the storage updates contain an operation the target's state-change handler does not \
         implement"
    ),
    known_revert!(
        sol_errors::InvalidArguments,
        "the target rejected the call arguments"
    ),
    known_revert!(
        sol_errors::RevertingContext,
        "a CALL replayed from the storage updates reverted inside the target"
    ),
    known_revert!(
        sol_errors::DeploymentFailed,
        "a CREATE or CREATE2 replayed from the storage updates failed"
    ),
    known_revert!(
        sol_errors::InvalidQuorumSignature,
        "the aggregate Schnorr signature does not verify against the registry's aggregate key at \
         the reference block"
    ),
    known_revert!(
        sol_errors::ReentrantTransition,
        "the target is already inside a tracked transition, so verifyAndUpdate cannot be entered"
    ),
    known_revert!(
        sol_errors::EmptyBatch,
        "the round produced no state updates to apply"
    ),
    known_revert!(
        sol_errors::BlockStaleMeasureOverflow,
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
    /// succeeded, so only an error the node produced by executing the call qualifies. Reverts
    /// that carry no return data are still reverts, and are recognised from the node's error
    /// response rather than from data it did not send.
    pub fn from_call_error(err: &alloy::contract::Error) -> Option<Self> {
        if let Some(data) = err.as_revert_data() {
            return Some(Self(data));
        }

        let alloy::contract::Error::TransportError(transport) = err else {
            return None;
        };
        let response = transport.as_error_resp()?;
        let reverted = response.code == EXECUTION_REVERTED_CODE
            || response.message.to_lowercase().contains("revert");
        reverted.then(|| Self(Bytes::new()))
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

/// Builds the contract error a node returns for a call that executed and reverted, with `data` as
/// the hex revert payload (`None` for a bare `revert()`).
///
/// Test-only: in production these only ever arrive from a provider, and both this module and the
/// executor need to drive the estimation path without a chain.
#[cfg(test)]
pub(crate) fn execution_reverted(data: Option<&str>) -> alloy::contract::Error {
    rpc_error_with_data(EXECUTION_REVERTED_CODE, "execution reverted", data)
}

/// Builds a JSON-RPC error response with no revert payload, for the failures that never reach
/// execution. Test-only, as with [`execution_reverted`].
#[cfg(test)]
pub(crate) fn rpc_error(code: i64, message: &'static str) -> alloy::contract::Error {
    rpc_error_with_data(code, message, None)
}

#[cfg(test)]
fn rpc_error_with_data(
    code: i64,
    message: &'static str,
    data: Option<&str>,
) -> alloy::contract::Error {
    use alloy::rpc::json_rpc::ErrorPayload;
    use alloy::transports::RpcError;
    use serde_json::value::RawValue;

    alloy::contract::Error::TransportError(RpcError::ErrorResp(ErrorPayload {
        code,
        message: message.into(),
        data: data.map(|d| RawValue::from_string(format!("\"{d}\"")).unwrap()),
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

    // Not every node uses code 3; the message is the fallback signal.
    #[test]
    fn a_revert_reported_under_a_provider_specific_code_is_recognised() {
        assert!(PayloadRevert::from_call_error(&rpc_error(-32000, "execution reverted")).is_some());
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
