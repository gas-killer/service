//! Submitter: turns certified heights into on-chain `verifyAndUpdate` calls.
//!
//! Consumes `(height, digest, certificate)` observations from the
//! [`crate::reporter::CertReporter`] and resolves each one:
//!
//! - `skip_digest(height)`: the quorum abandoned the height — log and notify the
//!   sequencer ([`ResolutionKind::Skipped`]); nothing goes on-chain.
//! - No assignment, or a digest that matches neither the assignment nor the skip
//!   digest: pre-restart leftovers — log and notify ([`ResolutionKind::Foreign`]);
//!   the sequencer re-assigns any in-flight task.
//! - Expected digest: pair the certificate's signer bitmap with its ECDSA
//!   signatures, sort by signer address (the on-chain dedupe order), and call
//!   [`GasKillerHandler::handle_verification`], with bounded retries.
//!
//! Unlike the BLS predecessor there is no on-chain read path here: the
//! certificate already carries everything `verifyAndUpdate` needs (the operators'
//! 65-byte `r || s || v` signatures), so no operator-registry lookups, stake
//! retrieval, or block pinning happen before submission.

use crate::executor::GasKillerHandler;
use crate::factories::SimpleWalletProvider;
use crate::reporter::{CertifiedHeight, CertifiedReceiver};
use crate::sequencer::{Resolution, ResolutionKind, ResolutionSender, SharedAssignments};
use alloy_primitives::{Bytes, FixedBytes};
use anyhow::Result;
use commonware_cryptography::sha256::Digest;
use gas_killer_common::EcdsaCertificate;
use gas_killer_common::ecdsa::EcdsaScheme;
use gas_killer_common::skip_digest;
use gas_killer_common::task_data::GasKillerTaskData;
use std::time::Duration;
use tracing::{error, info, warn};

/// Retries after the first failed submission attempt.
const MAX_RETRIES: u32 = 2;

/// Delay before the first retry; doubles per attempt.
const INITIAL_RETRY_BACKOFF: Duration = Duration::from_secs(2);

/// Consumes certified heights and drives the on-chain submission.
pub struct Submitter {
    /// Verifier scheme: maps certificate signer bitmaps to operator addresses.
    scheme: EcdsaScheme,
    /// Executes the final `verifyAndUpdate` transaction (multi-chain write side).
    handler: GasKillerHandler<SimpleWalletProvider>,
    /// Height → expected digest + task, written by the sequencer.
    assignments: SharedAssignments,
    /// Verified certificates from the reporter.
    certified: CertifiedReceiver,
    /// Final dispositions back to the sequencer.
    resolutions: ResolutionSender,
}

impl Submitter {
    pub fn new(
        scheme: EcdsaScheme,
        handler: GasKillerHandler<SimpleWalletProvider>,
        assignments: SharedAssignments,
        certified: CertifiedReceiver,
        resolutions: ResolutionSender,
    ) -> Self {
        Self {
            scheme,
            handler,
            assignments,
            certified,
            resolutions,
        }
    }

    /// Runs until the reporter side of the certified channel closes.
    pub async fn run(mut self) {
        while let Some(certified) = self.certified.recv().await {
            self.handle_certified(certified).await;
        }
        info!("certified channel closed; submitter exiting");
    }

    /// Notifies the sequencer of a height's final disposition.
    fn notify(&self, height: u64, kind: ResolutionKind) {
        if self.resolutions.send(Resolution { height, kind }).is_err() {
            // The sequencer is gone; the process is shutting down.
            warn!(height, "resolution channel closed");
        }
    }

    async fn handle_certified(&mut self, certified: CertifiedHeight) {
        let CertifiedHeight {
            height,
            digest,
            certificate,
        } = certified;

        // Skip certificate: the quorum abandoned the height.
        if digest == skip_digest(height) {
            info!(height, "skip certificate observed; nothing to submit");
            self.notify(height, ResolutionKind::Skipped);
            return;
        }

        // Look up the sequencer's assignment for this height.
        let assignment = match self.assignments.read() {
            Ok(assignments) => assignments.get(&height).cloned(),
            Err(_) => {
                error!(height, "assignments lock poisoned; treating as unassigned");
                None
            }
        };
        let Some(assignment) = assignment else {
            // Unassigned heights certify during journal replay (pre-restart
            // leftovers) or when nodes resolved a height from a previous router
            // life. There is nothing to submit — the task cache is gone.
            info!(
                height,
                digest = %digest,
                "certificate for unassigned height; nothing to submit"
            );
            self.notify(height, ResolutionKind::Foreign);
            return;
        };
        if assignment.digest != digest {
            // A certificate formed for a digest we did not announce (liveness
            // rule 5): the height is consumed, the in-flight task is not.
            warn!(
                height,
                expected = %assignment.digest,
                certified = %digest,
                "certificate digest does not match assignment; height consumed"
            );
            self.notify(height, ResolutionKind::Foreign);
            return;
        }

        // Expected digest: submit on-chain with bounded retries. Failures here
        // are either transient RPC issues (retry helps) or deterministic
        // rejections (retries burn two extra round-trips, then the height is
        // released as a failed execution).
        let mut backoff = INITIAL_RETRY_BACKOFF;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self
                .submit(height, digest, &certificate, &assignment.task)
                .await
            {
                Ok(result) => {
                    info!(
                        height,
                        tx = %result.transaction_hash,
                        block = ?result.block_number,
                        status = ?result.status,
                        "verifyAndUpdate submitted"
                    );
                    let success = result.status.unwrap_or(true);
                    self.notify(height, ResolutionKind::Executed { success });
                    return;
                }
                Err(error) if attempt <= MAX_RETRIES => {
                    warn!(
                        height,
                        attempt,
                        max_retries = MAX_RETRIES,
                        %error,
                        backoff_secs = backoff.as_secs_f64(),
                        "submission failed; retrying"
                    );
                    ::tokio::time::sleep(backoff).await;
                    backoff *= 2;
                }
                Err(error) => {
                    error!(
                        height,
                        attempts = attempt,
                        %error,
                        "submission failed after retries; releasing height"
                    );
                    self.notify(height, ResolutionKind::Executed { success: false });
                    return;
                }
            }
        }
    }

    /// One end-to-end submission attempt for a certified task digest.
    async fn submit(
        &mut self,
        height: u64,
        digest: Digest,
        certificate: &EcdsaCertificate,
        task: &GasKillerTaskData,
    ) -> Result<crate::executor::ExecutionResult> {
        // Bitmap → operator addresses, in participant order. The certificate was
        // verified by the reporter, so the bitmap length matches the set and the
        // signature list is index-aligned with the bitmap's ascending iteration.
        let signatures = ordered_signatures(&self.scheme, certificate)?;

        let msg_hash = FixedBytes::<32>::from_slice(digest.as_ref());
        info!(
            height,
            signers = signatures.len(),
            "submitting ECDSA quorum certificate"
        );

        self.handler
            .handle_verification(height, msg_hash, signatures, Some(task))
            .await
    }
}

/// Pairs a verified certificate's signer bitmap with its signatures and returns
/// the raw 65-byte signatures sorted by ascending signer address — the order
/// `GasKillerSDK.verifyAndUpdate` enforces to reject duplicate signers.
///
/// Participant order (the certificate's signature order) is already ascending by
/// address bytes, so this sort is a no-op in practice; it stays as a defensive
/// guarantee of the on-chain calling convention rather than an assumption about
/// the participant set's ordering.
fn ordered_signatures(scheme: &EcdsaScheme, certificate: &EcdsaCertificate) -> Result<Vec<Bytes>> {
    let addresses = scheme.signer_addresses(&certificate.signers);
    if addresses.is_empty() || addresses.len() != certificate.signatures.len() {
        return Err(anyhow::anyhow!(
            "signer bitmap resolved to an inconsistent signer set ({} addresses / {} signatures)",
            addresses.len(),
            certificate.signatures.len()
        ));
    }

    let mut pairs: Vec<_> = addresses
        .into_iter()
        .zip(certificate.signatures.iter())
        .collect();
    pairs.sort_by_key(|(address, _)| *address);

    Ok(pairs
        .into_iter()
        .map(|(_, signature)| Bytes::copy_from_slice(signature.as_ref()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;
    use commonware_consensus::aggregation::types::Item;
    use commonware_consensus::types::Height;
    use commonware_cryptography::certificate::Scheme as _;
    use commonware_cryptography::{Hasher as _, Sha256, Signer as _};
    use commonware_parallel::Sequential;
    use commonware_utils::ordered::Set;
    use commonware_utils::{N3f1, TryCollect};
    use gas_killer_common::ecdsa::{Ecdsa, PublicKey};

    /// Builds a 4-participant scheme set plus a verifier, participant-ordered.
    fn setup() -> (Vec<Ecdsa>, EcdsaScheme) {
        let keys: Vec<Ecdsa> = (0..4).map(Ecdsa::from_seed).collect();
        let participants: Set<PublicKey> = keys
            .iter()
            .map(|k| k.public_key())
            .try_collect()
            .expect("distinct keys");
        let verifier = EcdsaScheme::verifier(participants);
        (keys, verifier)
    }

    fn certificate_for(
        keys: &[Ecdsa],
        verifier: &EcdsaScheme,
        payload: &[u8],
    ) -> (EcdsaCertificate, [u8; 32]) {
        let mut hasher = Sha256::new();
        hasher.update(payload);
        let digest = hasher.finalize();
        let digest_bytes: [u8; 32] = digest.as_ref().try_into().unwrap();
        let subject = Item {
            height: Height::new(1),
            digest,
        };

        let schemes: Vec<EcdsaScheme> = keys
            .iter()
            .map(|k| EcdsaScheme::signer(verifier.participants().clone(), k.private_key()).unwrap())
            .collect();
        let attestations: Vec<_> = schemes
            .iter()
            .take(3)
            .map(|s| {
                s.sign::<commonware_cryptography::sha256::Digest>(&subject)
                    .unwrap()
            })
            .collect();
        let certificate = verifier
            .assemble::<_, N3f1>(attestations, &Sequential)
            .unwrap();
        (certificate, digest_bytes)
    }

    #[test]
    fn ordered_signatures_are_ascending_and_recover_to_operators() {
        let (keys, verifier) = setup();
        let (certificate, digest) = certificate_for(&keys, &verifier, b"submitter ordering");

        let signatures = ordered_signatures(&verifier, &certificate).unwrap();
        assert_eq!(signatures.len(), 3);

        // Recovered signer addresses are strictly ascending — the contract's
        // dedupe order — and every signer is one of the operators.
        let operators: Vec<Address> = verifier
            .participants()
            .iter()
            .map(|k| k.address())
            .collect();
        let mut last = Address::ZERO;
        for raw in &signatures {
            let sig = gas_killer_common::ecdsa::Signature::try_from(raw.as_ref()).unwrap();
            let recovered = sig.recover(&digest).unwrap();
            assert!(recovered > last, "signers must be strictly ascending");
            assert!(operators.contains(&recovered), "signer must be an operator");
            last = recovered;
        }
    }

    #[test]
    fn ordered_signatures_rejects_count_mismatch() {
        let (keys, verifier) = setup();
        let (mut certificate, _) = certificate_for(&keys, &verifier, b"count mismatch");
        certificate.signatures.pop();

        assert!(ordered_signatures(&verifier, &certificate).is_err());
    }
}
