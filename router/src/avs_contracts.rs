//! Resolves the contract addresses an integrator needs in order to settle, for
//! `GET /avs-metadata`.
//!
//! A target contract must be wired to two addresses: the AVS service manager and the
//! `IBLSSignatureChecker`. Get the checker wrong and the target still passes every router-side
//! check — it is handed a `ready` payload — and then reverts `InvalidQuorumApkHash` on chain,
//! because the router computes quorum APK indices against the live registry while the contract
//! checks them against a superseded one. Publishing the pair from the running deployment is what
//! makes that unwireable by accident.
//!
//! Nothing here is hand-copied, which is the point: hand-copied addresses are exactly what goes
//! stale when the operator set is redeployed.
//!
//! - `registryCoordinator` comes from the same `avs_deploy.json` the nodes read.
//! - `avsAddress` and `blsSignatureChecker` are read from a live target's own getters, so they are
//!   authoritative by construction: whatever a settling target verifies against *is* the answer.
//!   In particular the checker is not `avs_deploy.json`'s `blsSigCheck` — the target deploy script
//!   provisions a fresh checker from the registry coordinator, so that field names a different
//!   contract than any target actually uses.
//! - The two are then cross-checked: the checker's own `registryCoordinator()` must be the
//!   coordinator the operators are registered in. A mismatch means the reference target belongs to
//!   a superseded deployment, so nothing is published rather than publishing a pair that reverts.

use std::path::{Path, PathBuf};

use alloy_primitives::Address;
use alloy_provider::Provider;
use anyhow::Context;
use gas_killer_common::bindings::IBLSSignatureCheckerRegistry;
use gas_killer_common::bindings::gaskillersdk::GasKillerSDK;
use serde::{Deserialize, Serialize, Serializer};

/// Filename, alongside `avs_deploy.json`, holding the address of the target the deploy job last
/// provisioned. Used as the reference target when `AVS_REFERENCE_TARGET` is unset, so a deployment
/// that runs that job needs no configuration and cannot drift from what it deployed.
const DEMO_TARGET_FILENAME: &str = "demo_target.txt";

/// The addresses a target must be wired to in order to settle, plus the demo contracts the
/// documentation points at.
///
/// Serialized as the `contracts` object on `GET /avs-metadata`. Every field is a public on-chain
/// address, and the endpoint is already public and unauthenticated, so there is nothing here to
/// gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AvsContracts {
    /// EVM chain ID the addresses below live on, so an integrator can tell at a glance whether
    /// they are pointed at the right network.
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    /// AVS service manager a target names as its `avsAddress`.
    #[serde(rename = "avsAddress", serialize_with = "checksummed")]
    pub avs_address: Address,
    /// Signature checker a target must verify against. The verification-blocking one of the pair.
    #[serde(rename = "blsSignatureChecker", serialize_with = "checksummed")]
    pub bls_signature_checker: Address,
    /// Registry coordinator the operators are registered in. Published so an integrator can
    /// confirm their own wiring independently: `blsSignatureChecker().registryCoordinator()` must
    /// equal this.
    #[serde(rename = "registryCoordinator", serialize_with = "checksummed")]
    pub registry_coordinator: Address,
    /// A deployed target anyone may submit against, for a first settlement with no Solidity
    /// written. Shared, so concurrent readers can invalidate each other's payloads on
    /// `transitionIndex`; `demoFactory` is the way to avoid that. Absent when none is configured.
    #[serde(
        rename = "demoTarget",
        skip_serializing_if = "Option::is_none",
        serialize_with = "checksummed_opt"
    )]
    pub demo_target: Option<Address>,
    /// Factory that deploys a caller-owned target, for a reader who wants an instance no one else
    /// is advancing. It takes the AVS and checker addresses as arguments rather than wiring them
    /// itself, so a caller should pass the two published above. Absent when none is configured.
    #[serde(
        rename = "demoFactory",
        skip_serializing_if = "Option::is_none",
        serialize_with = "checksummed_opt"
    )]
    pub demo_factory: Option<Address>,
}

/// Serializes an address in EIP-55 checksummed form.
///
/// Solidity rejects a lowercase address literal outright — "this looks like an address but has an
/// invalid checksum" — and wiring a target's constructor is exactly what these addresses are for,
/// so the published form has to be one an integrator can paste straight into source. Alloy's own
/// `Serialize` emits lowercase, hence the override. Parsing is unaffected: either form reads back.
fn checksummed<S: Serializer>(address: &Address, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&address.to_checksum(None))
}

/// [`checksummed`] for an optional address. The `None` arm is unreachable while the field is
/// `skip_serializing_if`, but serde requires it.
fn checksummed_opt<S: Serializer>(
    address: &Option<Address>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match address {
        Some(address) => checksummed(address, serializer),
        None => serializer.serialize_none(),
    }
}

/// The subset of `avs_deploy.json` this module reads.
#[derive(Debug, Deserialize)]
struct AvsDeployment {
    addresses: AvsDeploymentAddresses,
}

#[derive(Debug, Deserialize)]
struct AvsDeploymentAddresses {
    /// Written lowercase by the deployer and capitalised by some older revisions; both name the
    /// same contract.
    #[serde(rename = "registryCoordinator", alias = "RegistryCoordinator")]
    registry_coordinator: Address,
}

/// Reads the registry coordinator out of an `avs_deploy.json`.
fn registry_coordinator_from(path: &Path) -> anyhow::Result<Address> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading AVS deployment at {}", path.display()))?;
    let deployment: AvsDeployment = serde_json::from_str(&raw)
        .with_context(|| format!("parsing AVS deployment at {}", path.display()))?;
    Ok(deployment.addresses.registry_coordinator)
}

/// The target whose getters the AVS and checker addresses are read from.
///
/// An `explicit` address (from `AVS_REFERENCE_TARGET`) wins. Failing that, the deploy job's own
/// record next to `avs_deploy.json` is used: that file is rewritten every time the job provisions a
/// target, so it tracks redeployments without anyone editing configuration. Returns `None` when
/// neither is available, which leaves the `contracts` block off rather than guessing.
fn reference_target(deployment_path: &Path, explicit: Option<Address>) -> Option<Address> {
    if let Some(address) = explicit {
        return Some(address);
    }
    let recorded = deployment_path.parent()?.join(DEMO_TARGET_FILENAME);
    std::fs::read_to_string(recorded).ok()?.trim().parse().ok()
}

/// Reads an optional address from an environment variable, ignoring a blank or malformed value.
fn address_from_env(key: &str) -> Option<Address> {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
}

/// Resolves the published contract set, or an error describing what could not be established.
///
/// Runs once at startup rather than per request: `/avs-metadata` is public and unauthenticated, so
/// serving it must not turn into an RPC amplifier, and these addresses only change when the AVS is
/// redeployed — which needs a router restart anyway to pick up the new `avs_deploy.json`.
pub async fn resolve<P: Provider>(
    provider: &P,
    deployment_path: &Path,
) -> anyhow::Result<AvsContracts> {
    let registry_coordinator = registry_coordinator_from(deployment_path)?;
    let target = reference_target(deployment_path, address_from_env("AVS_REFERENCE_TARGET"))
        .context(
        "no reference target to read the AVS and checker addresses from: set AVS_REFERENCE_TARGET",
    )?;

    let chain_id = provider
        .get_chain_id()
        .await
        .context("reading chain id for the published contract set")?;

    let sdk = GasKillerSDK::new(target, provider);
    let avs_address = sdk
        .avsAddress()
        .call()
        .await
        .with_context(|| format!("reading avsAddress() from reference target {target}"))?;
    let bls_signature_checker =
        sdk.blsSignatureChecker().call().await.with_context(|| {
            format!("reading blsSignatureChecker() from reference target {target}")
        })?;

    // The pairing check. A reference target left over from a superseded deployment reads back a
    // checker bound to that deployment's coordinator, and publishing it would hand every
    // integrator the wiring that reverts.
    let checker_coordinator = IBLSSignatureCheckerRegistry::new(bls_signature_checker, provider)
        .registryCoordinator()
        .call()
        .await
        .with_context(|| {
            format!("reading registryCoordinator() from checker {bls_signature_checker}")
        })?;
    if checker_coordinator != registry_coordinator {
        anyhow::bail!(
            "reference target {target} verifies against checker {bls_signature_checker}, whose \
             registry coordinator {checker_coordinator} is not the operators' coordinator \
             {registry_coordinator}: the target belongs to a superseded deployment"
        );
    }

    Ok(AvsContracts {
        chain_id,
        avs_address,
        bls_signature_checker,
        registry_coordinator,
        demo_target: address_from_env("DEMO_TARGET_ADDRESS"),
        demo_factory: address_from_env("DEMO_FACTORY_ADDRESS"),
    })
}

/// Path to `avs_deploy.json`, from `AVS_DEPLOYMENT_PATH`.
pub fn deployment_path() -> Option<PathBuf> {
    std::env::var("AVS_DEPLOYMENT_PATH").ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    /// The live testnet coordinator, in the lowercase form the deployer writes.
    const COORDINATOR_JSON: &str = r#"{
        "lastUpdate": { "timestamp": 1786464584, "block_number": 11467179 },
        "addresses": {
            "registryCoordinator": "0x0a032d62dde46670ae40ce532c97f6ce9af72dc4",
            "blsSigCheck": "0xc3BEF9ece3372c631c629D2D6E3cf51a9000A527"
        }
    }"#;

    fn write(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("writing fixture");
        path
    }

    #[test]
    fn reads_the_coordinator_from_a_deployment_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "avs_deploy.json", COORDINATOR_JSON);

        assert_eq!(
            registry_coordinator_from(&path).unwrap(),
            address!("0a032D62dde46670Ae40Ce532C97f6CE9Af72Dc4")
        );
    }

    #[test]
    fn accepts_the_capitalised_coordinator_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "avs_deploy.json",
            r#"{"addresses":{"RegistryCoordinator":"0x0a032d62dde46670ae40ce532c97f6ce9af72dc4"}}"#,
        );

        assert_eq!(
            registry_coordinator_from(&path).unwrap(),
            address!("0a032D62dde46670Ae40Ce532C97f6CE9Af72Dc4")
        );
    }

    #[test]
    fn a_deployment_without_a_coordinator_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "avs_deploy.json", r#"{"addresses":{}}"#);

        assert!(
            registry_coordinator_from(&path).is_err(),
            "a deployment file with no coordinator has no answer to publish"
        );
        assert!(
            registry_coordinator_from(&dir.path().join("absent.json")).is_err(),
            "a missing deployment file is an error, not a default"
        );
    }

    #[test]
    fn falls_back_to_the_target_the_deploy_job_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let deployment = write(dir.path(), "avs_deploy.json", COORDINATOR_JSON);
        // Written with a trailing newline by a shell redirect in some revisions of the job.
        write(
            dir.path(),
            DEMO_TARGET_FILENAME,
            "0xF143a9D93045474C2B573d21AC1CCe8dB2b06dbD\n",
        );

        assert_eq!(
            reference_target(&deployment, None),
            Some(address!("F143a9D93045474C2B573d21AC1CCe8dB2b06dbD")),
            "the job's own record tracks redeployments without anyone editing configuration"
        );
    }

    #[test]
    fn no_recorded_target_and_no_override_yields_none() {
        let dir = tempfile::tempdir().unwrap();
        let deployment = write(dir.path(), "avs_deploy.json", COORDINATOR_JSON);

        assert!(
            reference_target(&deployment, None).is_none(),
            "with nothing to read the addresses from, publish nothing rather than guess"
        );

        // An explicit override needs no recorded file at all.
        let explicit = address!("00000000000000000000000000000000000000aa");
        assert_eq!(
            reference_target(&deployment, Some(explicit)),
            Some(explicit)
        );
    }

    #[test]
    fn addresses_publish_in_checksummed_form() {
        let contracts = AvsContracts {
            chain_id: 11155111,
            avs_address: address!("dCec8ce0a03848B55989Bcc711e424Ca31d9eeD9"),
            bls_signature_checker: address!("6953fc47FC8b7568801f3fdc327bc0d9aD12E5b9"),
            registry_coordinator: address!("0a032D62dde46670Ae40Ce532C97f6CE9Af72Dc4"),
            demo_target: None,
            demo_factory: None,
        };

        let json = serde_json::to_value(&contracts).unwrap();
        // Solidity rejects a lowercase address literal, so a mixed-case rendering is the contract
        // with an integrator, not cosmetic.
        assert_eq!(
            json["avsAddress"],
            "0xdCec8ce0a03848B55989Bcc711e424Ca31d9eeD9"
        );
        assert_eq!(
            json["blsSignatureChecker"],
            "0x6953fc47FC8b7568801f3fdc327bc0d9aD12E5b9"
        );
        assert_eq!(
            json["registryCoordinator"],
            "0x0a032D62dde46670Ae40Ce532C97f6CE9Af72Dc4"
        );

        // Round-trips: a client that parses what it was served gets the same addresses back.
        let parsed: AvsContracts = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, contracts);
    }
}
