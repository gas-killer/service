//! Deploys the `SchnorrStakeRegistry` and registers the operator set against it.
//!
//! This is AVS operator-set construction, not contract deployment, which is why it is its own
//! binary rather than a mode of `deploy_example`. The arguments it submits are derived from
//! operator secret key material at runtime — a proof of possession per operator, generated with
//! a live RNG — so they cannot be expressed as manifest values the way a constructor's can.
//!
//! **It is a phase, and ordering is load-bearing.** Every registration advances the registry's
//! `effectiveBlock` watermark, and verification fail-closes for reference blocks behind it, so
//! the whole operator set must be registered *before* any target contract is deployed. Run this
//! first, then `deploy_example`, which reads the registry address back out of the deployment
//! JSON via `$deploy:schnorrStakeRegistry`.
//!
//! `SCHNORR_PROVISION` decides which phases run, and unset it follows `SIGNATURE_SCHEME`: the
//! whole thing under `schnorr`, nothing under `bls`, since the BLS stack verifies against a
//! `BLSSignatureChecker` from the EigenLayer deployment and needs none of this.
//!
//! | `SCHNORR_PROVISION` | Deploys and records | Registers |
//! |---|---|---|
//! | unset | only under `SIGNATURE_SCHEME=schnorr` | only under `SIGNATURE_SCHEME=schnorr` |
//! | `registry` | yes | no |
//! | `full` | yes | yes |
//!
//! `registry` is for a deployment that wants the address published before it has an operator set
//! to put in it. A target's constructor takes that address, so an integrator can wire one and
//! settle under whichever scheme the fleet is running; the registry verifies nothing until
//! operators are registered, which its owner can do later into the same registry. Both explicit
//! values run whatever scheme the fleet signs, which is the point of them.
//!
//! `SCHNORR_STAKE_REGISTRY_ADDRESS` reuses a deployed registry instead. Registering into one is
//! only done while it holds nothing but this operator set; see [`FillPlan`].

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use gas_killer_common::avs_contracts::SCHNORR_STAKE_REGISTRY_KEY;
use gas_killer_common::schnorr::{PrivateKey, private_key_from_hex};
use gas_killer_common::{
    SignatureScheme, quorum_threshold_fraction, schnorr_notice_window, signature_scheme,
};
use rand::RngCore;
use scripts::bindings::schnorrstakeregistry::SchnorrStakeRegistry;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

type DynError = Box<dyn std::error::Error + Send + Sync>;

/// Operator key files the eigenlayer setup container writes next to the deployment JSON. The
/// Schnorr signing key IS the operator's secp256k1 key.
const OPERATOR_KEY_FILE_SUFFIX: &str = ".private.ecdsa.key.json";

#[derive(Debug, Deserialize)]
struct OperatorKeyFile {
    #[serde(rename = "privateKey")]
    private_key: String,
}

#[tokio::main]
async fn main() -> Result<(), DynError> {
    dotenv::dotenv().ok();

    let provision = parse_provision(env::var("SCHNORR_PROVISION").ok().as_deref())?;
    let (deploy_registry, register) = provision.phases(signature_scheme());
    if !deploy_registry {
        println!(
            "⏭️  SIGNATURE_SCHEME is not 'schnorr' and SCHNORR_PROVISION is unset; nothing to do \
             (the BLS stack verifies against a BLSSignatureChecker from the EigenLayer \
             deployment). Set SCHNORR_PROVISION=registry to provision one anyway."
        );
        return Ok(());
    }

    let http_rpc = env::var("HTTP_RPC").map_err(|_| "HTTP_RPC environment variable is required")?;
    let private_key =
        env::var("PRIVATE_KEY").map_err(|_| "PRIVATE_KEY environment variable is required")?;
    let avs_deployment_path = env::var("AVS_DEPLOYMENT_PATH")
        .map_err(|_| "AVS_DEPLOYMENT_PATH environment variable is required")?;

    // The on-chain stake threshold, fixed at registry deployment. Must match the router
    // coordinator's local participation floor (same env vars, see `quorum_threshold_fraction`).
    let (threshold_num, threshold_den) = quorum_threshold_fraction();

    // Blocks an operator-set change must be announced ahead of applying, fixed at registry
    // deployment. Zero for the e2e stack: the whole operator set is registered before any target
    // deploys, so no round is ever in flight for a mutation to invalidate.
    let notice_window = schnorr_notice_window();

    // The AVS reference comes from the eigenlayer deployment JSON — operators register with
    // EigenLayer through the same service manager regardless of the quorum-signature scheme the
    // target contract verifies.
    let avs_address = read_avs_address(&avs_deployment_path)?;

    // The operators' Schnorr keys are their existing secp256k1 keys, read from the key files the
    // eigenlayer setup container produced. Loaded before the registry exists, so a volume missing
    // them fails with the reason rather than after deploying a registry nobody is in.
    let operator_keys = if register {
        let keys = load_operator_keys(&avs_deployment_path)?;
        println!("🔑 Loaded {} operator key(s)", keys.len());
        keys
    } else {
        println!("📋 Deploying the registry without registering an operator set");
        Vec::new()
    };

    // The deployer owns the registry (stand-in for the EigenLayer registration lifecycle).
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|_| "Invalid private key format")?;
    let deployer = signer.address();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(signer))
        .connect_http(http_rpc.parse().map_err(|_| "Invalid RPC URL")?);

    let code_avs = provider
        .get_code_at(avs_address)
        .await
        .map_err(|e| format!("Failed to get code for AVS address {avs_address}: {e}"))?;
    if code_avs.is_empty() {
        return Err(format!(
            "AVS service manager {avs_address} has no code deployed. Check AVS_DEPLOYMENT_PATH."
        )
        .into());
    }

    // Reuse SCHNORR_STAKE_REGISTRY_ADDRESS when it points at deployed code, otherwise deploy a
    // fresh registry.
    let (registry_address, reused) = match env_address("SCHNORR_STAKE_REGISTRY_ADDRESS")? {
        Some(addr) => {
            let code = provider
                .get_code_at(addr)
                .await
                .map_err(|e| format!("Failed to get code for SchnorrStakeRegistry {addr}: {e}"))?;
            if code.is_empty() {
                return Err(format!(
                    "SCHNORR_STAKE_REGISTRY_ADDRESS {addr} has no code deployed; unset it to \
                     deploy a fresh registry"
                )
                .into());
            }
            println!("🏦 Using existing SchnorrStakeRegistry at: {addr}");
            (addr, true)
        }
        None => {
            println!(
                "🏦 Deploying SchnorrStakeRegistry (threshold {threshold_num}/{threshold_den}, \
                 owner {deployer}, notice window {notice_window} blocks)..."
            );
            let registry = SchnorrStakeRegistry::deploy(
                provider.clone(),
                U256::from(threshold_num),
                U256::from(threshold_den),
                deployer,
                U256::from(notice_window),
            )
            .await
            .map_err(|e| format!("Failed to deploy SchnorrStakeRegistry: {e}"))?;
            let address = *registry.address();
            println!("✅ SchnorrStakeRegistry deployed at: {address}");
            (address, false)
        }
    };

    if !register {
        println!(
            "⏭️  No operator registered. The registry verifies nothing until one is, and its \
             owner {deployer} is who can register them."
        );
    } else if reused {
        fill_reused_registry(&provider, registry_address, deployer, &operator_keys).await?;
    } else {
        register_operator_set(&provider, registry_address, &operator_keys).await?;
    }

    record_registry_address(&avs_deployment_path, registry_address)?;

    println!("\n🎉 Schnorr operator set ready");
    println!("  SchnorrStakeRegistry: {registry_address}");
    println!("  AVS service manager:  {avs_address}");
    println!(
        "\nNext: deploy a target, which reads the registry via \
         $deploy:{SCHNORR_STAKE_REGISTRY_KEY}\n  \
         cargo run -p scripts --bin deploy_example -- --example schnorrArraySummation"
    );
    Ok(())
}

/// Registers every operator's Schnorr key with a fresh proof of possession.
///
/// MUST complete before any target deploys: the registry's `effectiveBlock` watermark advances
/// on every registration, and verification fail-closes for reference blocks behind it.
async fn register_operator_set<P: Provider + Clone>(
    provider: &P,
    registry_address: Address,
    operator_keys: &[PrivateKey],
) -> Result<(), DynError> {
    let registry = SchnorrStakeRegistry::new(registry_address, provider.clone());
    let mut rng = rand::rng();
    let mut fill = |b: &mut [u8]| rng.fill_bytes(b);

    for key in operator_keys {
        let pubkey = key.public_key();
        let operator = pubkey.eth_address();
        let pop = key.prove_possession(&mut fill);
        // Cheap local check before spending gas: the registry verifies the same PoP.
        if !pubkey.verify_possession(&pop) {
            return Err(format!(
                "locally generated proof of possession failed to verify for operator {operator}"
            )
            .into());
        }
        let pop_bytes = pop.0.to_bytes();
        let receipt = registry
            .registerOperator(
                U256::from_be_bytes(pubkey.x_bytes()),
                U256::from_be_bytes(pubkey.y_bytes()),
                // Uniform weight: the e2e stack has no stake differentiation.
                U256::from(1),
                U256::from_be_slice(&pop_bytes[..32]),
                Address::from_slice(&pop_bytes[32..]),
            )
            .send()
            .await
            .map_err(|e| format!("Failed to send registerOperator for {operator}: {e}"))?
            .get_receipt()
            .await
            .map_err(|e| {
                format!("registerOperator transaction for {operator} failed or was not mined: {e}")
            })?;
        if !receipt.status() {
            return Err(format!("registerOperator reverted for operator {operator}").into());
        }
        println!("✅ Registered operator {operator} (weight 1)");
    }
    Ok(())
}

/// What a reused registry needs, from what it already holds.
#[derive(Debug, PartialEq, Eq)]
enum FillPlan {
    /// It holds exactly this operator set.
    Complete,
    /// It holds part of this operator set or none of it, and nothing else. The forced path can
    /// then only invalidate a round assembled against this fleet's own partial set, which a retry
    /// recovers, and completing a partial set is what lets a run that died mid-fill be re-run.
    Register,
    /// It holds weight that is not this deployment's, or has changes scheduled. Either is a set
    /// someone else is managing, through the announce path, and a foreign operator would sit in
    /// the quorum as a permanent non-signer.
    Refuse,
}

/// What a reused registry holds, as far as [`FillPlan`] is concerned.
struct RegistryState {
    /// This deployment's operators not yet registered.
    missing: usize,
    /// Weight registered to this deployment's operators.
    own_weight: U256,
    total_weight: U256,
    pending_changes: U256,
}

impl FillPlan {
    fn for_registry(state: &RegistryState) -> Self {
        if state.total_weight != state.own_weight || !state.pending_changes.is_zero() {
            Self::Refuse
        } else if state.missing == 0 {
            Self::Complete
        } else {
            Self::Register
        }
    }
}

/// Registers the operator set into a reused registry when [`FillPlan`] allows it.
async fn fill_reused_registry<P: Provider + Clone>(
    provider: &P,
    registry_address: Address,
    deployer: Address,
    operator_keys: &[PrivateKey],
) -> Result<(), DynError> {
    let registry = SchnorrStakeRegistry::new(registry_address, provider.clone());

    let mut missing = Vec::new();
    let mut own_weight = U256::ZERO;
    for key in operator_keys {
        let operator = key.public_key().eth_address();
        let record = registry.operators(operator).call().await.map_err(|e| {
            format!("Failed to read operator {operator} from {registry_address}: {e}")
        })?;
        if record.registered {
            own_weight += U256::from(record.weight);
        } else {
            missing.push(key.clone());
        }
    }
    let total_weight = registry
        .totalWeight()
        .call()
        .await
        .map_err(|e| format!("Failed to read totalWeight from {registry_address}: {e}"))?;
    let pending_changes =
        registry.pendingChangeCount().call().await.map_err(|e| {
            format!("Failed to read pendingChangeCount from {registry_address}: {e}")
        })?;
    let state = RegistryState {
        missing: missing.len(),
        own_weight,
        total_weight,
        pending_changes,
    };

    match FillPlan::for_registry(&state) {
        FillPlan::Complete => {
            println!(
                "✅ All {} operator(s) already registered (registry total weight {total_weight})",
                operator_keys.len()
            );
            Ok(())
        }
        FillPlan::Register => {
            // Checked here because the revert would only say NotOwner.
            let owner = registry
                .owner()
                .call()
                .await
                .map_err(|e| format!("Failed to read owner of {registry_address}: {e}"))?;
            if owner != deployer {
                return Err(format!(
                    "SchnorrStakeRegistry {registry_address} is owned by {owner}, not the deployer \
                     {deployer}; only its owner can register the operator set"
                )
                .into());
            }
            println!(
                "📋 Registering {} of {} operator(s)",
                missing.len(),
                operator_keys.len()
            );
            register_operator_set(provider, registry_address, &missing).await
        }
        FillPlan::Refuse => Err(format!(
            "SchnorrStakeRegistry {registry_address} holds weight {total_weight}, of which \
             {own_weight} is this deployment's, with {pending_changes} change(s) scheduled. A \
             registry holding other operators or scheduled changes is managed through \
             announceRegister and commitNextChange, not this job."
        )
        .into()),
    }
}

/// Loads every operator's secp256k1 key from the `*.private.ecdsa.key.json` files the eigenlayer
/// setup container writes next to the deployment JSON (override the directory with
/// `OPERATOR_KEYS_DIR`). Sorted by filename for a deterministic registration order.
fn load_operator_keys(avs_deployment_path: &str) -> Result<Vec<PrivateKey>, DynError> {
    let keys_dir: PathBuf = match env::var("OPERATOR_KEYS_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(avs_deployment_path)
            .parent()
            .map(|p| p.join("operator_keys"))
            .unwrap_or_else(|| PathBuf::from("operator_keys")),
    };
    println!("🔑 Loading operator keys from: {}", keys_dir.display());

    let mut key_files: Vec<PathBuf> = fs::read_dir(&keys_dir)
        .map_err(|e| {
            format!(
                "Failed to read operator keys directory {}: {e}",
                keys_dir.display()
            )
        })?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(OPERATOR_KEY_FILE_SUFFIX))
        })
        .collect();
    key_files.sort();

    if key_files.is_empty() {
        return Err(format!(
            "no *{OPERATOR_KEY_FILE_SUFFIX} files found in {} — the eigenlayer setup container \
             writes them next to the deployment JSON (or set OPERATOR_KEYS_DIR)",
            keys_dir.display()
        )
        .into());
    }

    let mut keys = Vec::with_capacity(key_files.len());
    for file in &key_files {
        let content = fs::read_to_string(file)
            .map_err(|e| format!("Failed to read operator key file {}: {e}", file.display()))?;
        let parsed: OperatorKeyFile = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse operator key file {}: {e}", file.display()))?;
        let key = private_key_from_hex(&parsed.private_key)
            .ok_or_else(|| format!("Invalid privateKey in operator key file {}", file.display()))?;
        keys.push(key);
    }
    Ok(keys)
}

/// Reads `addresses.avsServiceManagerWrapper` from the eigenlayer deployment JSON.
fn read_avs_address(avs_deployment_path: &str) -> Result<Address, DynError> {
    println!("📖 Reading AVS deployment from: {avs_deployment_path}");
    let content = fs::read_to_string(avs_deployment_path)
        .map_err(|e| format!("Failed to read AVS deployment file: {e}"))?;
    let parsed: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse AVS deployment JSON: {e}"))?;
    let raw = parsed
        .get("addresses")
        .and_then(|a| a.get("avsServiceManagerWrapper"))
        .and_then(|v| v.as_str())
        .ok_or("addresses.avsServiceManagerWrapper missing from the deployment JSON")?;
    raw.parse()
        .map_err(|_| format!("Invalid avsServiceManagerWrapper address: {raw}").into())
}

/// Records the registry under `addresses.schnorrStakeRegistry`, preserving every other key.
/// This is the handoff to `deploy_example`, which resolves it as `$deploy:schnorrStakeRegistry`.
fn record_registry_address(avs_deployment_path: &str, registry: Address) -> Result<(), DynError> {
    let content = fs::read_to_string(avs_deployment_path)
        .map_err(|e| format!("Failed to read deployment JSON for updating: {e}"))?;
    let mut deployment: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse deployment JSON for updating: {e}"))?;

    if !deployment["addresses"].is_object() {
        deployment["addresses"] = serde_json::json!({});
    }
    deployment["addresses"][SCHNORR_STAKE_REGISTRY_KEY] =
        serde_json::json!(format!("{registry:?}"));

    let serialized = serde_json::to_string_pretty(&deployment)
        .map_err(|e| format!("Failed to serialize deployment JSON: {e}"))?;
    fs::write(avs_deployment_path, serialized)
        .map_err(|e| format!("Failed to write deployment JSON: {e}"))?;
    println!("📝 recorded addresses.{SCHNORR_STAKE_REGISTRY_KEY} = {registry:?}");
    Ok(())
}

/// Which phases `SCHNORR_PROVISION` asks for. Mirrors the chart's `schnorr.provision`, which
/// passes its value straight through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provision {
    /// Unset: the phases follow `SIGNATURE_SCHEME`.
    Scheme,
    /// Deploy and record the registry, register nobody.
    Registry,
    /// Deploy, record, and register the operator set.
    Full,
}

impl Provision {
    /// Whether to deploy and record the registry, and whether to register the operator set.
    fn phases(self, scheme: SignatureScheme) -> (bool, bool) {
        match self {
            Self::Scheme => {
                let schnorr = scheme == SignatureScheme::Schnorr;
                (schnorr, schnorr)
            }
            Self::Registry => (true, false),
            Self::Full => (true, true),
        }
    }
}

/// Parses `SCHNORR_PROVISION`, treating unset and empty alike.
///
/// An unrecognized value is an error rather than a fallback. Falling back would either skip the
/// registrations or run them, and both are wrong to guess at: one leaves a registry that verifies
/// nothing, and the other submits transactions the caller did not ask for.
fn parse_provision(raw: Option<&str>) -> Result<Provision, DynError> {
    match raw.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") => Ok(Provision::Scheme),
        Some("registry") => Ok(Provision::Registry),
        Some("full") => Ok(Provision::Full),
        Some(other) => Err(format!(
            "SCHNORR_PROVISION must be \"registry\" or \"full\" (or unset to follow \
             SIGNATURE_SCHEME), got {other:?}"
        )
        .into()),
    }
}

fn env_address(name: &str) -> Result<Option<Address>, DynError> {
    match env::var(name).ok().filter(|s| !s.trim().is_empty()) {
        Some(raw) => {
            Ok(Some(raw.trim().parse().map_err(|_| {
                format!("Invalid address in {name}: {raw}")
            })?))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{FillPlan, Provision, RegistryState, parse_provision};
    use alloy::primitives::U256;
    use gas_killer_common::SignatureScheme;

    fn state(missing: usize, own: u64, total: u64, pending: u64) -> RegistryState {
        RegistryState {
            missing,
            own_weight: U256::from(own),
            total_weight: U256::from(total),
            pending_changes: U256::from(pending),
        }
    }

    #[test]
    fn a_reused_registry_is_filled_only_while_it_holds_nothing_else() {
        assert_eq!(
            FillPlan::for_registry(&state(3, 0, 0, 0)),
            FillPlan::Register
        );
        assert_eq!(
            FillPlan::for_registry(&state(2, 1, 1, 0)),
            FillPlan::Register,
            "a fill that died partway must be resumable"
        );
        assert_eq!(
            FillPlan::for_registry(&state(0, 3, 3, 0)),
            FillPlan::Complete
        );
        assert_eq!(
            FillPlan::for_registry(&state(0, 3, 5, 0)),
            FillPlan::Refuse,
            "a superset leaves a foreign operator in the quorum"
        );
        assert_eq!(
            FillPlan::for_registry(&state(3, 0, 3, 0)),
            FillPlan::Refuse,
            "another deployment's operator set must not be joined through the forced path"
        );
        assert_eq!(
            FillPlan::for_registry(&state(3, 0, 0, 1)),
            FillPlan::Refuse,
            "a scheduled change means someone else is managing the set"
        );
    }

    #[test]
    fn an_unset_provision_follows_the_signature_scheme() {
        for raw in [None, Some(""), Some("  ")] {
            let provision = parse_provision(raw).expect("unset is valid");
            assert_eq!(provision, Provision::Scheme);
            assert_eq!(provision.phases(SignatureScheme::Schnorr), (true, true));
            assert_eq!(
                provision.phases(SignatureScheme::Bls),
                (false, false),
                "a bls fleet that asked for nothing must not deploy a registry"
            );
        }
    }

    #[test]
    fn an_explicit_provision_runs_under_either_scheme() {
        for raw in ["registry", "REGISTRY", " Registry "] {
            let provision = parse_provision(Some(raw)).expect("registry is valid");
            assert_eq!(provision.phases(SignatureScheme::Bls), (true, false));
            assert_eq!(provision.phases(SignatureScheme::Schnorr), (true, false));
        }
        for raw in ["full", "FULL", " Full "] {
            let provision = parse_provision(Some(raw)).expect("full is valid");
            assert_eq!(provision.phases(SignatureScheme::Bls), (true, true));
            assert_eq!(provision.phases(SignatureScheme::Schnorr), (true, true));
        }
    }

    #[test]
    fn a_mistyped_provision_is_an_error_rather_than_a_guess() {
        for raw in ["registry-only", "true", "1", "yes", "none", "off"] {
            let err = parse_provision(Some(raw))
                .expect_err("a value that is neither must not be guessed at");
            assert!(
                format!("{err}").contains("SCHNORR_PROVISION"),
                "the error must name the variable, got {err}"
            );
        }
    }
}
