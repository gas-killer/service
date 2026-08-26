//! Resolves the contract addresses that make up a deployment's settlement wiring.
//!
//! A target contract must be wired to two addresses: the AVS service manager and the
//! `IBLSSignatureChecker`. Get the checker wrong and the target still passes every router-side
//! check — it is handed a `ready` payload — and then reverts `InvalidQuorumApkHash` on chain,
//! because the router computes quorum APK indices against the live registry while the contract
//! checks them against a superseded one. Publishing the pair from the running deployment is what
//! makes that unwireable by accident.
//!
//! The set is shared because the wiring is: the router aggregates certificates against the
//! operators' registry coordinator and submits to a target that verifies through the checker, and a
//! node validates work for that same deployment. The router also publishes the resolved set as the
//! `contracts` block on `GET /avs-metadata`, which is what saves an integrator from hand-copying
//! any of it — hand-copied addresses are exactly what goes stale when the operator set is
//! redeployed.
//!
//! - `registryCoordinator` comes from the same `avs_deploy.json` loader the rest of the service
//!   reads, so there is one parser for that file.
//! - `avsAddress` and `blsSignatureChecker` are read from a live target's own getters, so they are
//!   authoritative by construction: whatever a settling target verifies against *is* the answer.
//!   In particular the checker is not `avs_deploy.json`'s `blsSigCheck` — upstream maps that key
//!   onto the operator-state retriever, and the target deploy script provisions a fresh checker
//!   from the registry coordinator, so that field names a different contract than any target uses.
//! - The two are then cross-checked: the checker's own `registryCoordinator()` must be the
//!   coordinator the operators are registered in. A mismatch means the reference target belongs to
//!   a superseded deployment, so nothing is published rather than publishing a pair that reverts.
//! - `demoTarget` and `demoFactory` come from configuration or, failing that, from what the
//!   playground job recorded. The playground target doubles as the preferred reference target, so
//!   the checker published is the one `demoTarget.blsSignatureChecker()` itself returns.
//!
//! Resolution costs four RPC round-trips, so the answer is cached in a [`ResolvedContracts`] slot
//! rather than recomputed per use: `/avs-metadata` is public and unauthenticated, and serving it
//! must not turn into an RPC amplifier. Because the addresses only change when the AVS is
//! redeployed — which restarts the process — the slot is written at most once, and a failed attempt
//! is retried in the background until it succeeds.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::bindings::IBLSSignatureCheckerRegistry;
use crate::bindings::gaskillersdk::GasKillerSDK;
use alloy_primitives::Address;
use alloy_provider::Provider;
use anyhow::Context;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tracing::{error, info, warn};

/// Filename holding the address of the target the deploy job last provisioned. Used as the
/// reference target when no pin and no record path are configured, looked for beside
/// `avs_deploy.json`.
const DEMO_TARGET_FILENAME: &str = "demo_target.txt";

/// Names the file recording the target the deploy job provisioned.
///
/// Needed because that record and `avs_deploy.json` are not siblings on every deployment: under
/// Secret Manager the router reads the deployment file from a secrets volume holding nothing else,
/// while the deploy job writes its record to the shared-data volume. Where the chart sets this, the
/// job is what produces the file, so its absence means "not written yet" and resolution is retried.
const REFERENCE_TARGET_FILE_ENV: &str = "AVS_REFERENCE_TARGET_FILE";

/// Names the file recording the playground target — the shared contract the documentation points
/// readers at, deployed by its own job so public traffic cannot advance the transition counter the
/// smoke-test target depends on. Set for the same reason as [`REFERENCE_TARGET_FILE_ENV`]: the job
/// writes to the shared-data volume, which is not where `avs_deploy.json` is read from.
const DEMO_TARGET_FILE_ENV: &str = "DEMO_TARGET_FILE";

/// Names the file recording the playground `ArraySummationFactory`, from which a reader deploys a
/// target only they are advancing.
const DEMO_FACTORY_FILE_ENV: &str = "DEMO_FACTORY_FILE";

/// How long one resolution attempt may run before it is abandoned and retried. Four sequential RPC
/// round-trips sit behind it, and this runs during startup, so an unresponsive provider must not
/// stall the router indefinitely.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(20);

/// Delay before the first retry of a failed resolution, doubled on each further failure up to
/// [`RETRY_MAX_BACKOFF`].
const RETRY_INITIAL_BACKOFF: Duration = Duration::from_secs(5);

/// Ceiling on the retry backoff. Kept to a minute because one thing being waited on is the target
/// deploy job, which can finish minutes after the router is already serving.
const RETRY_MAX_BACKOFF: Duration = Duration::from_secs(60);

/// The addresses a target must be wired to in order to settle, plus the demo contracts the
/// documentation points at.
///
/// Serialized as the `contracts` object on `GET /avs-metadata`. Every field is a public on-chain
/// address, and the endpoint is already public and unauthenticated, so there is nothing here to
/// gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AvsContracts {
    /// EVM chain ID every address below lives on, so an integrator can tell at a glance whether
    /// they are pointed at the right network. True of the AVS trio by construction — they are read
    /// through a provider for this chain — and of the demo contracts because each is checked for
    /// code here before being published.
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

/// The slot the resolved contract set is published into, shared between the background resolver and
/// every `/avs-metadata` response.
///
/// Write-once: these addresses change only on a redeployment, which the router has to restart to
/// pick up anyway, so the resolver fills the slot on its first success and stops. A `OnceLock`
/// rather than a lock means the request path reads it without blocking or poisoning.
#[derive(Debug, Clone, Default)]
pub struct ResolvedContracts(Arc<OnceLock<AvsContracts>>);

impl ResolvedContracts {
    /// The resolved set, or `None` while resolution has not yet succeeded.
    pub fn get(&self) -> Option<&AvsContracts> {
        self.0.get()
    }

    /// Whether nothing has been resolved yet. Drives `skip_serializing_if` so an unresolved set is
    /// absent from the response rather than serialized as `null` — a client branches on presence.
    pub fn is_unresolved(&self) -> bool {
        self.0.get().is_none()
    }

    /// Publishes the resolved set. A second call is ignored: the slot is write-once.
    pub fn publish(&self, contracts: AvsContracts) {
        let _ = self.0.set(contracts);
    }
}

impl Serialize for ResolvedContracts {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0.get() {
            Some(contracts) => contracts.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for ResolvedContracts {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let contracts = Option::<AvsContracts>::deserialize(deserializer)?;
        let slot = Self::default();
        if let Some(contracts) = contracts {
            slot.publish(contracts);
        }
        Ok(slot)
    }
}

/// Where the reference target and the demo contracts come from, read from the environment once so
/// that resolution itself is a pure function of its inputs.
#[derive(Debug, Clone, Default)]
pub struct ContractsConfig {
    /// `AVS_REFERENCE_TARGET`: pins the target whose getters establish the AVS/checker pair.
    pub reference_target: Option<Address>,
    /// [`REFERENCE_TARGET_FILE_ENV`]: file the deploy job records that target in.
    pub reference_target_file: Option<PathBuf>,
    /// `DEMO_TARGET_ADDRESS`.
    pub demo_target: Option<Address>,
    /// [`DEMO_TARGET_FILE_ENV`]: file the playground job records its target in.
    pub demo_target_file: Option<PathBuf>,
    /// `DEMO_FACTORY_ADDRESS`.
    pub demo_factory: Option<Address>,
    /// [`DEMO_FACTORY_FILE_ENV`]: file the playground job records its factory in.
    pub demo_factory_file: Option<PathBuf>,
}

impl ContractsConfig {
    /// Reads the configuration, failing on a malformed `AVS_REFERENCE_TARGET`.
    ///
    /// A set-but-unparseable pin is an error rather than a silent `None`: it would otherwise fall
    /// through to the deploy job's record and publish a *different* pair than the operator asked
    /// for, on an endpoint whose whole purpose is that nobody reads a wrong address. The demo
    /// contracts are documentation pointers, so a typo there warns and omits the field instead of
    /// withholding the settlement addresses.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            reference_target: address_from_env("AVS_REFERENCE_TARGET")?,
            reference_target_file: non_empty_env(REFERENCE_TARGET_FILE_ENV).map(PathBuf::from),
            demo_target: demo_address_from_env("DEMO_TARGET_ADDRESS"),
            demo_target_file: non_empty_env(DEMO_TARGET_FILE_ENV).map(PathBuf::from),
            demo_factory: demo_address_from_env("DEMO_FACTORY_ADDRESS"),
            demo_factory_file: non_empty_env(DEMO_FACTORY_FILE_ENV).map(PathBuf::from),
        })
    }
}

/// An environment variable's value, treating unset and blank alike.
fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

/// Parses an address, naming the variable it came from so a typo is actionable from the log line.
fn parse_address(key: &str, raw: &str) -> anyhow::Result<Address> {
    raw.parse()
        .with_context(|| format!("{key} is not an EVM address: {raw:?}"))
}

/// Reads an optional address from an environment variable. Blank is unset; malformed is an error.
fn address_from_env(key: &str) -> anyhow::Result<Option<Address>> {
    non_empty_env(key)
        .map(|raw| parse_address(key, &raw))
        .transpose()
}

/// [`address_from_env`] for a demo contract, degraded to `None` with a warning on a malformed
/// value so a documentation pointer's typo cannot suppress the settlement addresses.
fn demo_address_from_env(key: &str) -> Option<Address> {
    match address_from_env(key) {
        Ok(address) => address,
        Err(e) => {
            warn!(
                error = %e,
                "ignoring a malformed demo contract address; /avs-metadata will omit it"
            );
            None
        }
    }
}

/// Reads an address a deploy job recorded in a file.
///
/// Trailing whitespace is tolerated: the jobs write with `printf`, but a shell redirect elsewhere
/// may leave a newline. Content that is not an address is an error rather than an omission — a job
/// that failed mid-write, or wrote a message where an address should be, must not resolve to
/// "nothing configured".
fn read_recorded_address(path: &Path) -> anyhow::Result<Address> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading the address recorded at {}", path.display()))?;
    parse_address(
        &format!("the address recorded at {}", path.display()),
        raw.trim(),
    )
}

/// An address configured outright, or failing that the one a deploy job recorded.
///
/// The configured value wins so a deployment can point readers at something other than what its own
/// job deployed. A named record that cannot be read is an error, not an omission: the job that
/// writes it can finish after the router is already serving, so the attempt is retried.
fn recorded_address(
    configured: Option<Address>,
    record: Option<&Path>,
) -> anyhow::Result<Option<Address>> {
    if let Some(address) = configured {
        return Ok(Some(address));
    }
    record.map(read_recorded_address).transpose()
}

/// The target whose getters the AVS and checker addresses are read from.
///
/// The `AVS_REFERENCE_TARGET` pin wins. Failing that a deploy job's own record is read, which tracks
/// redeployments without anyone editing configuration: the playground record, then the smoke-test
/// one, then — for a layout that keeps everything on one volume — a record beside `avs_deploy.json`.
///
/// The playground target comes first because it is the contract the documentation points readers at,
/// and it is published as `demoTarget`. Each target deploy provisions its own signature checker, so
/// reading the pair off anything else would publish a checker that verifies correctly yet is not the
/// one `demoTarget.blsSignatureChecker()` returns — a discrepancy an integrator would reasonably
/// read as a bug. Preferring it makes the published set describe the very contract a reader submits
/// against.
///
/// `Ok(None)` means nothing is configured to read the pair from, so the block is left off for good.
/// An error means a source that should exist could not be read, and resolution is retried: a named
/// record is written by a job that can finish after the router starts serving.
fn reference_target(
    deployment_path: &Path,
    config: &ContractsConfig,
) -> anyhow::Result<Option<Address>> {
    if let Some(address) = config.reference_target {
        return Ok(Some(address));
    }
    if let Some(path) = config
        .demo_target_file
        .as_deref()
        .or(config.reference_target_file.as_deref())
    {
        return read_recorded_address(path).map(Some);
    }
    let Some(sibling) = deployment_path
        .parent()
        .map(|dir| dir.join(DEMO_TARGET_FILENAME))
    else {
        return Ok(None);
    };
    if !sibling.exists() {
        return Ok(None);
    }
    read_recorded_address(&sibling).map(Some)
}

/// Keeps a demo address only if it has code on the chain the block publishes.
///
/// The demo contracts are free-form configuration, so unlike the AVS trio nothing about them proves
/// they are on this chain — an address could name a contract on another network, or be
/// checksum-valid and simply wrong. Checking for code is what makes `chainId` true of the whole
/// block. An RPC failure propagates so the attempt is retried rather than publishing a partial set.
async fn deployed_on_chain<P: Provider>(
    provider: &P,
    field: &str,
    address: Option<Address>,
) -> anyhow::Result<Option<Address>> {
    let Some(address) = address else {
        return Ok(None);
    };
    let code = provider
        .get_code_at(address)
        .await
        .with_context(|| format!("reading code at the configured {field} {address}"))?;
    if code.is_empty() {
        warn!(
            %address,
            field,
            "no code at the configured demo contract on this chain; /avs-metadata will omit it"
        );
        return Ok(None);
    }
    Ok(Some(address))
}

/// Resolves the published contract set.
///
/// `Ok(None)` means there is no reference target to read the pair from, so there is nothing to
/// publish and nothing to retry. An error is a failed attempt: something that should have answered
/// did not, or the pair that came back is not the operators' own.
pub async fn resolve<P: Provider>(
    provider: &P,
    registry_coordinator: Address,
    deployment_path: &Path,
    config: &ContractsConfig,
) -> anyhow::Result<Option<AvsContracts>> {
    let Some(target) = reference_target(deployment_path, config)? else {
        return Ok(None);
    };

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

    Ok(Some(AvsContracts {
        chain_id,
        avs_address,
        bls_signature_checker,
        registry_coordinator,
        demo_target: deployed_on_chain(
            provider,
            "demoTarget",
            recorded_address(config.demo_target, config.demo_target_file.as_deref())?,
        )
        .await?,
        demo_factory: deployed_on_chain(
            provider,
            "demoFactory",
            recorded_address(config.demo_factory, config.demo_factory_file.as_deref())?,
        )
        .await?,
    }))
}

/// Fills `slot` in the background, retrying a failed resolution until it succeeds.
///
/// A failure is not fatal — the rest of `/avs-metadata` is identity information worth serving, and
/// an absent block reads as "no authoritative answer" rather than handing out a wrong one — but it
/// must not be permanent either. The providers this reads through are known to flap, which is why
/// the router carries an RPC circuit breaker, and the deploy job that records the reference target
/// can finish after the router is already serving. Without a retry either would leave the block
/// absent for the process lifetime, self-healing only on another restart.
///
/// The loop ends on the first success, and on `Ok(None)`, which means nothing is configured to
/// resolve from. Anything else keeps retrying with backoff, so a misconfiguration stays visible in
/// the logs instead of scrolling past once at startup.
pub fn spawn_resolver<P: Provider + 'static>(
    provider: P,
    registry_coordinator: Address,
    deployment_path: PathBuf,
    config: ContractsConfig,
    slot: ResolvedContracts,
) {
    tokio::spawn(async move {
        let mut backoff = RETRY_INITIAL_BACKOFF;
        loop {
            let attempt = resolve(&provider, registry_coordinator, &deployment_path, &config);
            match tokio::time::timeout(RESOLVE_TIMEOUT, attempt).await {
                Ok(Ok(Some(contracts))) => {
                    info!(
                        avs_address = %contracts.avs_address,
                        bls_signature_checker = %contracts.bls_signature_checker,
                        registry_coordinator = %contracts.registry_coordinator,
                        chain_id = contracts.chain_id,
                        "publishing settlement contract addresses on /avs-metadata"
                    );
                    slot.publish(contracts);
                    return;
                }
                Ok(Ok(None)) => {
                    info!(
                        "no reference target configured ({REFERENCE_TARGET_FILE_ENV} or \
                         AVS_REFERENCE_TARGET); /avs-metadata will omit the contracts block"
                    );
                    return;
                }
                Ok(Err(e)) => error!(
                    error = %e,
                    retry_in_secs = backoff.as_secs(),
                    "could not resolve settlement contract addresses; /avs-metadata omits them \
                     until this succeeds"
                ),
                Err(_) => error!(
                    timeout_secs = RESOLVE_TIMEOUT.as_secs(),
                    retry_in_secs = backoff.as_secs(),
                    "timed out resolving settlement contract addresses; /avs-metadata omits them \
                     until this succeeds"
                ),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(RETRY_MAX_BACKOFF);
        }
    });
}

/// Path to `avs_deploy.json`, from `AVS_DEPLOYMENT_PATH`. Used only to locate the deploy job's
/// record beside it; the file's own contents are read through the shared loader.
pub fn deployment_path() -> Option<PathBuf> {
    non_empty_env("AVS_DEPLOYMENT_PATH").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolValue;
    use alloy_primitives::{Bytes, U64, address};
    use alloy_provider::{ProviderBuilder, mock::Asserter};

    const TARGET: Address = address!("F143a9D93045474C2B573d21AC1CCe8dB2b06dbD");
    const AVS: Address = address!("dCec8ce0a03848B55989Bcc711e424Ca31d9eeD9");
    const CHECKER: Address = address!("6953fc47FC8b7568801f3fdc327bc0d9aD12E5b9");
    const COORDINATOR: Address = address!("0a032D62dde46670Ae40Ce532C97f6CE9Af72Dc4");
    const SUPERSEDED_COORDINATOR: Address = address!("00000000000000000000000000000000000000bb");
    const DEMO: Address = address!("00000000000000000000000000000000000000aa");
    const PLAYGROUND: Address = address!("00000000000000000000000000000000000000cc");
    const FACTORY: Address = address!("00000000000000000000000000000000000000dd");

    fn write(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("writing fixture");
        path
    }

    // -- reference target selection --

    #[test]
    fn a_pinned_target_needs_no_recorded_file() {
        let config = ContractsConfig {
            reference_target: Some(TARGET),
            ..Default::default()
        };

        assert_eq!(
            reference_target(Path::new("/nonexistent/avs_deploy.json"), &config).unwrap(),
            Some(TARGET)
        );
    }

    #[test]
    fn reads_the_target_from_the_named_record() {
        let dir = tempfile::tempdir().unwrap();
        // Written with a trailing newline by a shell redirect in some revisions of the job.
        let recorded = write(dir.path(), DEMO_TARGET_FILENAME, &format!("{TARGET}\n"));
        let config = ContractsConfig {
            reference_target_file: Some(recorded),
            ..Default::default()
        };

        assert_eq!(
            reference_target(Path::new("/nonexistent/avs_deploy.json"), &config).unwrap(),
            Some(TARGET),
            "the job's own record tracks redeployments without anyone editing configuration"
        );
    }

    #[test]
    fn a_named_record_that_is_absent_is_retried_not_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let config = ContractsConfig {
            reference_target_file: Some(dir.path().join(DEMO_TARGET_FILENAME)),
            ..Default::default()
        };

        assert!(
            reference_target(Path::new("/nonexistent/avs_deploy.json"), &config).is_err(),
            "the deploy job writes that file and can finish after the router is serving"
        );
    }

    #[test]
    fn falls_back_to_a_record_beside_the_deployment_file() {
        let dir = tempfile::tempdir().unwrap();
        let deployment = write(dir.path(), "avs_deploy.json", "{}");
        write(dir.path(), DEMO_TARGET_FILENAME, &TARGET.to_string());

        assert_eq!(
            reference_target(&deployment, &ContractsConfig::default()).unwrap(),
            Some(TARGET),
            "a deployment keeping both files on one volume needs no extra configuration"
        );
    }

    #[test]
    fn the_playground_record_wins_over_the_smoke_record() {
        let dir = tempfile::tempdir().unwrap();
        let config = ContractsConfig {
            demo_target_file: Some(write(
                dir.path(),
                "playground_target.txt",
                &format!("{PLAYGROUND}\n"),
            )),
            reference_target_file: Some(write(
                dir.path(),
                DEMO_TARGET_FILENAME,
                &TARGET.to_string(),
            )),
            ..Default::default()
        };

        // Reading the pair off the contract the docs point at is what keeps the published
        // blsSignatureChecker equal to demoTarget.blsSignatureChecker().
        assert_eq!(
            reference_target(Path::new("/nonexistent/avs_deploy.json"), &config).unwrap(),
            Some(PLAYGROUND)
        );

        let pinned = ContractsConfig {
            reference_target: Some(TARGET),
            ..config
        };
        assert_eq!(
            reference_target(Path::new("/nonexistent/avs_deploy.json"), &pinned).unwrap(),
            Some(TARGET),
            "an operator's pin outranks both records"
        );
    }

    #[test]
    fn the_smoke_record_is_the_reference_when_no_playground_runs() {
        let dir = tempfile::tempdir().unwrap();
        let config = ContractsConfig {
            reference_target_file: Some(write(
                dir.path(),
                DEMO_TARGET_FILENAME,
                &TARGET.to_string(),
            )),
            ..Default::default()
        };

        assert_eq!(
            reference_target(Path::new("/nonexistent/avs_deploy.json"), &config).unwrap(),
            Some(TARGET)
        );
    }

    #[test]
    fn demo_contracts_fall_back_to_the_playground_records() {
        let dir = tempfile::tempdir().unwrap();
        let target = write(dir.path(), "playground_target.txt", &PLAYGROUND.to_string());
        let factory = write(dir.path(), "playground_factory.txt", &FACTORY.to_string());

        assert_eq!(
            recorded_address(None, Some(&target)).unwrap(),
            Some(PLAYGROUND)
        );
        assert_eq!(
            recorded_address(None, Some(&factory)).unwrap(),
            Some(FACTORY)
        );
        assert_eq!(
            recorded_address(Some(DEMO), Some(&target)).unwrap(),
            Some(DEMO),
            "a configured address points readers somewhere other than what the job deployed"
        );
        assert_eq!(
            recorded_address(None, None).unwrap(),
            None,
            "with no playground deployed the demo fields are omitted, not defaulted"
        );
    }

    #[test]
    fn a_record_holding_something_other_than_an_address_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        // A job that failed mid-write, or wrote an error message where an address should be.
        let record = write(dir.path(), "playground_target.txt", "ERROR: forge failed\n");

        assert!(
            recorded_address(None, Some(&record)).is_err(),
            "a malformed record must be retried, not read as nothing configured"
        );
    }

    #[test]
    fn nothing_configured_yields_none() {
        let dir = tempfile::tempdir().unwrap();
        let deployment = write(dir.path(), "avs_deploy.json", "{}");

        assert!(
            reference_target(&deployment, &ContractsConfig::default())
                .unwrap()
                .is_none(),
            "with nothing to read the addresses from, publish nothing rather than guess"
        );
    }

    #[test]
    fn a_malformed_address_is_an_error_not_a_silent_none() {
        assert!(
            parse_address("AVS_REFERENCE_TARGET", "0xnot-an-address").is_err(),
            "a typo must not fall through to a different source and publish another pair"
        );
        assert_eq!(
            parse_address("AVS_REFERENCE_TARGET", &TARGET.to_string()).unwrap(),
            TARGET
        );
    }

    // -- resolution against a mocked provider --
    //
    // Responses are queued FIFO and consumed by each RPC call in the order `resolve` makes them:
    //   1. eth_chainId
    //   2. eth_call   avsAddress()
    //   3. eth_call   blsSignatureChecker()
    //   4. eth_call   registryCoordinator() on the checker
    //   5. eth_getCode demoTarget, then demoFactory, each only when configured

    fn mock_provider() -> (impl Provider + Clone, Asserter) {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        (provider, asserter)
    }

    fn push_address(asserter: &Asserter, address: Address) {
        asserter.push_success(&Bytes::from(address.abi_encode()));
    }

    /// Queues the four reads of a healthy deployment, with `checker_coordinator` as what the
    /// checker reports.
    fn push_wiring(asserter: &Asserter, checker_coordinator: Address) {
        asserter.push_success(&U64::from(11155111u64));
        push_address(asserter, AVS);
        push_address(asserter, CHECKER);
        push_address(asserter, checker_coordinator);
    }

    fn pinned(demo_target: Option<Address>) -> ContractsConfig {
        ContractsConfig {
            reference_target: Some(TARGET),
            demo_target,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn publishes_the_pair_a_live_target_verifies_against() {
        let (provider, asserter) = mock_provider();
        push_wiring(&asserter, COORDINATOR);

        let contracts = resolve(
            &provider,
            COORDINATOR,
            Path::new("/nonexistent/avs_deploy.json"),
            &pinned(None),
        )
        .await
        .unwrap()
        .expect("a pinned reference target has a set to publish");

        assert_eq!(contracts.chain_id, 11155111);
        assert_eq!(contracts.avs_address, AVS);
        assert_eq!(contracts.bls_signature_checker, CHECKER);
        assert_eq!(contracts.registry_coordinator, COORDINATOR);
    }

    #[tokio::test]
    async fn rejects_a_checker_bound_to_another_coordinator() {
        let (provider, asserter) = mock_provider();
        push_wiring(&asserter, SUPERSEDED_COORDINATOR);

        let err = resolve(
            &provider,
            COORDINATOR,
            Path::new("/nonexistent/avs_deploy.json"),
            &pinned(None),
        )
        .await
        .expect_err("a superseded pair must not be published");

        // This is the load-bearing check: the pair reads back cleanly and only the cross-check
        // tells it apart from a working one.
        let message = format!("{err}");
        assert!(
            message.contains("superseded deployment"),
            "expected the pairing check to reject it, got {message}"
        );
    }

    #[tokio::test]
    async fn resolves_to_nothing_without_a_reference_target() {
        let dir = tempfile::tempdir().unwrap();
        let deployment = write(dir.path(), "avs_deploy.json", "{}");
        let (provider, _asserter) = mock_provider();

        // No responses are queued: nothing to read the pair from means no RPC is made at all.
        assert!(
            resolve(
                &provider,
                COORDINATOR,
                &deployment,
                &ContractsConfig::default()
            )
            .await
            .unwrap()
            .is_none()
        );
    }

    #[tokio::test]
    async fn publishes_a_demo_contract_that_has_code() {
        let (provider, asserter) = mock_provider();
        push_wiring(&asserter, COORDINATOR);
        asserter.push_success(&Bytes::from(vec![0x60u8]));

        let contracts = resolve(
            &provider,
            COORDINATOR,
            Path::new("/nonexistent/avs_deploy.json"),
            &pinned(Some(DEMO)),
        )
        .await
        .unwrap()
        .expect("a pinned reference target has a set to publish");

        assert_eq!(contracts.demo_target, Some(DEMO));
    }

    #[tokio::test]
    async fn omits_a_demo_contract_with_no_code_on_this_chain() {
        let (provider, asserter) = mock_provider();
        push_wiring(&asserter, COORDINATOR);
        asserter.push_success(&Bytes::new());

        let contracts = resolve(
            &provider,
            COORDINATOR,
            Path::new("/nonexistent/avs_deploy.json"),
            &pinned(Some(DEMO)),
        )
        .await
        .unwrap()
        .expect("a demo address on the wrong chain must not withhold the settlement pair");

        assert!(
            contracts.demo_target.is_none(),
            "chainId is published for every address in the block, so an address that is not \
             there cannot be one of them"
        );
    }

    // -- serialization --

    #[test]
    fn addresses_publish_in_checksummed_form() {
        let contracts = AvsContracts {
            chain_id: 11155111,
            avs_address: AVS,
            bls_signature_checker: CHECKER,
            registry_coordinator: COORDINATOR,
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

    #[test]
    fn an_unresolved_slot_serializes_as_null_and_reports_itself_unresolved() {
        let slot = ResolvedContracts::default();
        assert!(slot.is_unresolved());
        assert_eq!(
            serde_json::to_value(&slot).unwrap(),
            serde_json::Value::Null
        );

        slot.publish(AvsContracts {
            chain_id: 11155111,
            avs_address: AVS,
            bls_signature_checker: CHECKER,
            registry_coordinator: COORDINATOR,
            demo_target: None,
            demo_factory: None,
        });
        assert!(!slot.is_unresolved());
        assert_eq!(
            serde_json::to_value(&slot).unwrap()["avsAddress"],
            "0xdCec8ce0a03848B55989Bcc711e424Ca31d9eeD9"
        );
    }
}
