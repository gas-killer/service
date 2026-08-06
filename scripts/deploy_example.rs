//! Deploys contracts from the public `gas-killer/example-contracts` library and points the
//! AVS at them.
//!
//! Unlike `deploy_array_summation`, nothing here is specific to one contract. Constructor
//! argument *types* are read from the Foundry artifact's ABI at runtime and the *values* come
//! from a TOML manifest, so adding an example is a manifest edit — no new Rust, no
//! recompile. Fetch and build the artifacts first with `scripts/examples/fetch_examples.sh`.
//!
//! Each example goes through the same sequence:
//!
//! 1. Resolve the AVS service manager and the BLS signature checker the target's constructor
//!    needs, and *validate the checker* — see [`validate_sig_checker`] for why this matters
//!    more than it looks.
//! 2. Deploy, by appending ABI-encoded constructor args to the artifact's creation bytecode.
//! 3. Assert the deployed target is actually routable: it must pass the same ERC-165 gate the
//!    router applies before submitting `verifyAndUpdate`, and expose the state-tracking and
//!    digest getters the aggregation path calls. Catching this here turns a confusing
//!    mid-round router error into one line at deploy time.
//! 4. Run the manifest's setup calls, which put the contract in a state where its expensive
//!    function is meaningful (depositors for a vault, and so on).
//! 5. Record the address under `addresses.<name>` in the AVS deployment JSON and emit a
//!    ready-to-run scenario file.
//!
//! Step 5 is what makes the target reachable by the rest of the tooling: `run_scenario`
//! resolves `target_address = "local:<name>"` against exactly that JSON, so no other
//! component needs to learn about the new contract.

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{Address, Bytes, FixedBytes, U256, keccak256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy_dyn_abi::{DynSolType, DynSolValue, JsonAbiExt, Specifier};
use alloy_json_abi::{Function, JsonAbi};
use clap::Parser;
use gas_killer_common::bindings::gaskillersdk::GasKillerSDK;
use gas_killer_common::bindings::{GAS_KILLER_INTERFACE_ID, SCHNORR_GAS_KILLER_INTERFACE_ID};
use gas_killer_common::{SignatureScheme, signature_scheme};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

type DynError = Box<dyn std::error::Error + Send + Sync>;

/// Key in the AVS deployment JSON holding the service manager the target registers against.
const AVS_ADDRESS_KEY: &str = "avsServiceManagerWrapper";

/// Key in the AVS deployment JSON holding a contract that really implements
/// `BLSSignatureChecker.checkSignatures`. The local EigenLayer stack's task manager inherits
/// `BLSSignatureChecker`, which is why it — and not `blsSigCheck` — is the default.
const SIG_CHECKER_ADDRESS_KEY: &str = "IncredibleSquaringTaskManager";

/// Key in the AVS deployment JSON holding the `BLSSigCheckOperatorStateRetriever`. It is the
/// router's off-chain helper for assembling non-signer stakes, *not* a signature checker: it
/// has no `checkSignatures`, so a target constructed with it reverts with an empty `0x` the
/// first time a quorum tries to settle. Rejected by name rather than diagnosed later.
const RETRIEVER_ADDRESS_KEY: &str = "blsSigCheck";

/// Address substituted for an unresolvable `$avs`/`$sigChecker` under `--dry-run`, where the
/// point is to validate manifest encoding rather than on-chain wiring.
const DRY_RUN_PLACEHOLDER: Address = Address::repeat_byte(0xee);

#[derive(Parser, Debug)]
#[command(
    about = "Deploy example-contracts targets and register them for the local Gas Killer stack"
)]
struct Cli {
    /// Manifest describing the examples, their constructor values, and setup calls.
    #[arg(long, default_value = "scripts/examples/examples.toml")]
    manifest: PathBuf,

    /// Example to deploy by `name`; repeatable. Defaults to every example in the manifest.
    #[arg(long = "example")]
    examples: Vec<String>,

    /// Directory holding the Foundry `out/` tree built from the example contracts.
    /// Defaults to `$EXAMPLES_DIR/out`, then `.examples/example-contracts/out`.
    #[arg(long)]
    artifacts: Option<PathBuf>,

    /// AVS deployment JSON to read wiring from and record deployed addresses into.
    /// Defaults to `$AVS_DEPLOYMENT_PATH`, then `config/.nodes/avs_deploy.json`.
    #[arg(long)]
    deploy_json: Option<PathBuf>,

    /// AVS service manager address, overriding env and the deployment JSON.
    #[arg(long)]
    avs: Option<Address>,

    /// BLS signature checker address, overriding env and the deployment JSON.
    #[arg(long)]
    sig_checker: Option<Address>,

    /// Router URL written into the generated scenario files.
    #[arg(long)]
    router_url: Option<String>,

    /// Directory the generated scenario files are written to.
    #[arg(long, default_value = "scripts/scenarios/generated")]
    scenario_dir: PathBuf,

    /// Reuse the address already recorded for an example instead of deploying a second copy.
    #[arg(long)]
    reuse: bool,

    /// Resolve, encode, and emit scenarios without sending any transaction.
    #[arg(long)]
    dry_run: bool,
}

// ---------------------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    /// Private keys available to setup calls. Index 0 is the deployer and the default sender
    /// for every call; defaults to `["$env:PRIVATE_KEY"]`.
    #[serde(default)]
    signers: Vec<String>,
    #[serde(default)]
    examples: Vec<ExampleSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExampleSpec {
    /// Key this contract is recorded under in the deployment JSON, and the name used for
    /// `--example` and the generated scenario file.
    name: String,
    /// Artifact to deploy, as `File.sol:Contract` or just `Contract`.
    artifact: String,
    #[serde(default)]
    ctor_args: Vec<ArgValue>,
    /// Calls made after deployment to put the contract in a usable state.
    #[serde(default)]
    setup: Vec<CallSpec>,
    /// The tracked functions to drive through the AVS; one scenario request each.
    #[serde(default)]
    exercise: Vec<ExerciseSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallSpec {
    /// Human-readable signature, e.g. `deposit(uint256)`.
    sig: String,
    #[serde(default)]
    args: Vec<ArgValue>,
    /// Index into the manifest's `signers`; defaults to the deployer.
    #[serde(default)]
    signer: usize,
    /// Wei to attach, decimal or `0x`-prefixed.
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExerciseSpec {
    /// Label for the generated scenario request; defaults to the example name.
    #[serde(default)]
    label: Option<String>,
    sig: String,
    #[serde(default)]
    args: Vec<ArgValue>,
    /// Whether the generated request polls `stateTransitionCount()` afterwards.
    #[serde(default = "default_true")]
    verify: bool,
}

fn default_true() -> bool {
    true
}

/// A manifest argument value: either a scalar to coerce against the ABI type, or a list
/// mapping onto an array or tuple parameter.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ArgValue {
    Scalar(String),
    List(Vec<ArgValue>),
}

// ---------------------------------------------------------------------------------------
// Placeholder resolution
// ---------------------------------------------------------------------------------------

/// Everything needed to expand the `$…` placeholders a manifest may reference. Holds plain
/// data so resolution is testable without a chain or a filesystem.
#[derive(Debug, Default)]
struct Resolver {
    avs: Option<Address>,
    sig_checker: Option<Address>,
    /// `addresses` from the AVS deployment JSON.
    deploy_addresses: BTreeMap<String, String>,
    signers: Vec<Address>,
}

impl Resolver {
    /// Expands a single scalar. Values not starting with `$` pass through untouched.
    ///
    /// - `$avs` / `$sigChecker` — the resolved constructor wiring
    /// - `$deploy:<key>` — any key under `addresses` in the deployment JSON
    /// - `$signer:<n>` — the address of the nth manifest signer
    /// - `$env:VAR` — an environment variable
    fn resolve(&self, raw: &str) -> Result<String, DynError> {
        let Some(rest) = raw.strip_prefix('$') else {
            return Ok(raw.to_string());
        };

        if let Some(key) = rest.strip_prefix("deploy:") {
            return self.deploy_addresses.get(key).cloned().ok_or_else(|| {
                format!("$deploy:{key} not found under `addresses` in the deployment JSON").into()
            });
        }
        if let Some(idx) = rest.strip_prefix("signer:") {
            let idx: usize = idx
                .parse()
                .map_err(|_| format!("$signer:{idx} is not a signer index"))?;
            return self
                .signers
                .get(idx)
                .map(|a| format!("{a:?}"))
                .ok_or_else(|| {
                    format!(
                        "$signer:{idx} is out of range; the manifest declares {} signer(s)",
                        self.signers.len()
                    )
                    .into()
                });
        }
        if let Some(var) = rest.strip_prefix("env:") {
            return std::env::var(var)
                .map_err(|_| format!("$env:{var} is not set in the environment").into());
        }

        match rest {
            "avs" => self
                .avs
                .map(|a| format!("{a:?}"))
                .ok_or_else(|| avs_unresolved_message(AVS_ADDRESS_KEY, "--avs", "EXAMPLE_AVS_ADDRESS").into()),
            "sigChecker" => self.sig_checker.map(|a| format!("{a:?}")).ok_or_else(|| {
                avs_unresolved_message(
                    SIG_CHECKER_ADDRESS_KEY,
                    "--sig-checker",
                    "EXAMPLE_SIG_CHECKER_ADDRESS",
                )
                .into()
            }),
            other => Err(format!(
                "unknown placeholder `${other}`; supported: $avs, $sigChecker, $deploy:<key>, $signer:<n>, $env:VAR"
            )
            .into()),
        }
    }
}

fn avs_unresolved_message(json_key: &str, flag: &str, env_var: &str) -> String {
    format!(
        "could not resolve the address for `{json_key}`: pass {flag}, set {env_var}, or point \
         --deploy-json at a deployment JSON that contains it"
    )
}

/// Recursively expands placeholders inside a manifest argument.
fn resolve_arg(resolver: &Resolver, arg: &ArgValue) -> Result<ArgValue, DynError> {
    match arg {
        ArgValue::Scalar(s) => Ok(ArgValue::Scalar(resolver.resolve(s)?)),
        ArgValue::List(items) => Ok(ArgValue::List(
            items
                .iter()
                .map(|i| resolve_arg(resolver, i))
                .collect::<Result<Vec<_>, _>>()?,
        )),
    }
}

// ---------------------------------------------------------------------------------------
// ABI coercion
// ---------------------------------------------------------------------------------------

/// Turns a manifest value into a `DynSolValue` of the type the ABI declares.
///
/// Array, fixed-array, and tuple parameters recurse into a TOML list; anything else is
/// coerced from its string form, which is also how a bracketed array literal is accepted.
fn coerce_arg(ty: &DynSolType, arg: &ArgValue) -> Result<DynSolValue, DynError> {
    match (ty, arg) {
        (DynSolType::Array(inner), ArgValue::List(items)) => Ok(DynSolValue::Array(
            items
                .iter()
                .map(|i| coerce_arg(inner, i))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        (DynSolType::FixedArray(inner, len), ArgValue::List(items)) => {
            if items.len() != *len {
                return Err(format!(
                    "expected {len} values for {ty}, manifest supplied {}",
                    items.len()
                )
                .into());
            }
            Ok(DynSolValue::FixedArray(
                items
                    .iter()
                    .map(|i| coerce_arg(inner, i))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        (DynSolType::Tuple(types), ArgValue::List(items)) => {
            if items.len() != types.len() {
                return Err(format!(
                    "expected {} values for {ty}, manifest supplied {}",
                    types.len(),
                    items.len()
                )
                .into());
            }
            Ok(DynSolValue::Tuple(
                types
                    .iter()
                    .zip(items)
                    .map(|(t, i)| coerce_arg(t, i))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        (_, ArgValue::Scalar(s)) => ty
            .coerce_str(s)
            .map_err(|e| format!("`{s}` is not a valid {ty}: {e}").into()),
        (_, ArgValue::List(_)) => {
            Err(format!("{ty} takes a single value, manifest supplied a list").into())
        }
    }
}

/// Coerces a positional argument list against the ABI types it will be encoded as.
fn coerce_args(types: &[DynSolType], args: &[ArgValue]) -> Result<Vec<DynSolValue>, DynError> {
    if types.len() != args.len() {
        return Err(format!(
            "expected {} argument(s), manifest supplied {}",
            types.len(),
            args.len()
        )
        .into());
    }
    types
        .iter()
        .zip(args)
        .map(|(t, a)| coerce_arg(t, a))
        .collect()
}

/// Parses a human-readable signature (`step(uint32)`) and ABI-encodes a call to it.
fn encode_call(sig: &str, args: &[ArgValue]) -> Result<Bytes, DynError> {
    let func = parse_signature(sig)?;
    let types = resolve_param_types(&func.inputs)?;
    let values = coerce_args(&types, args).map_err(|e| format!("encoding call to `{sig}`: {e}"))?;
    Ok(func
        .abi_encode_input(&values)
        .map_err(|e| format!("failed to ABI-encode `{sig}`: {e}"))?
        .into())
}

/// Parses a signature, tolerating a leading `function ` keyword.
fn parse_signature(sig: &str) -> Result<Function, DynError> {
    let trimmed = sig.trim();
    let body = trimmed.strip_prefix("function ").unwrap_or(trimmed);
    Function::parse(body)
        .map_err(|e| format!("`{sig}` is not a valid function signature: {e}").into())
}

/// Resolves ABI parameter declarations into the dynamic types used for encoding.
fn resolve_param_types<P: Specifier<DynSolType>>(
    params: &[P],
) -> Result<Vec<DynSolType>, DynError> {
    params
        .iter()
        .map(|p| {
            p.resolve()
                .map_err(|e| -> DynError { format!("unsupported ABI parameter type: {e}").into() })
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Forge artifacts
// ---------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ForgeArtifact {
    abi: JsonAbi,
    bytecode: ForgeBytecode,
}

#[derive(Debug, Deserialize)]
struct ForgeBytecode {
    object: String,
}

/// Maps an `artifact` manifest entry onto its path in a Foundry `out/` tree.
///
/// `File.sol:Contract` selects both explicitly; a bare `Contract` assumes the conventional
/// `Contract.sol/Contract.json`.
fn artifact_path(artifacts_dir: &Path, artifact: &str) -> PathBuf {
    match artifact.split_once(':') {
        Some((file, contract)) => artifacts_dir.join(file).join(format!("{contract}.json")),
        None => artifacts_dir
            .join(format!("{artifact}.sol"))
            .join(format!("{artifact}.json")),
    }
}

/// Reads an artifact and returns its ABI plus creation bytecode.
///
/// An artifact whose `bytecode.object` is empty is interface-only (the shape the service
/// vendors for `GasKillerSDK`) and cannot be deployed.
fn load_artifact(path: &Path) -> Result<(JsonAbi, Bytes), DynError> {
    let raw = fs::read_to_string(path).map_err(|e| {
        format!(
            "failed to read artifact {}: {e} — run scripts/examples/fetch_examples.sh first",
            path.display()
        )
    })?;
    let artifact: ForgeArtifact = serde_json::from_str(&raw)
        .map_err(|e| format!("failed to parse artifact {}: {e}", path.display()))?;

    let hex_body = artifact.bytecode.object.trim_start_matches("0x");
    if hex_body.is_empty() {
        return Err(format!(
            "artifact {} carries no creation bytecode; it is ABI-only and cannot be deployed",
            path.display()
        )
        .into());
    }
    let bytecode = alloy::hex::decode(hex_body)
        .map_err(|e| format!("artifact {} has invalid bytecode: {e}", path.display()))?;

    Ok((artifact.abi, bytecode.into()))
}

/// Builds the creation transaction input: creation bytecode with ABI-encoded constructor
/// arguments appended.
fn build_init_code(abi: &JsonAbi, bytecode: &Bytes, args: &[ArgValue]) -> Result<Bytes, DynError> {
    let mut init_code = bytecode.to_vec();

    match &abi.constructor {
        Some(ctor) => {
            let types = resolve_param_types(&ctor.inputs)?;
            let values = coerce_args(&types, args)
                .map_err(|e| format!("encoding constructor arguments: {e}"))?;
            let encoded = ctor
                .abi_encode_input(&values)
                .map_err(|e| format!("failed to ABI-encode constructor arguments: {e}"))?;
            init_code.extend_from_slice(&encoded);
        }
        None if args.is_empty() => {}
        None => {
            return Err(format!(
                "artifact has no constructor but the manifest supplies {} argument(s)",
                args.len()
            )
            .into());
        }
    }

    Ok(init_code.into())
}

// ---------------------------------------------------------------------------------------
// Deployment JSON
// ---------------------------------------------------------------------------------------

/// Reads `addresses` out of an AVS deployment JSON, tolerating a missing file so a testnet
/// run can start from nothing.
fn read_deploy_addresses(path: &Path) -> Result<BTreeMap<String, String>, DynError> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Ok(BTreeMap::new());
    };
    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("failed to parse deployment JSON {}: {e}", path.display()))?;

    let Some(addresses) = parsed.get("addresses").and_then(|a| a.as_object()) else {
        return Ok(BTreeMap::new());
    };
    Ok(addresses
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect())
}

/// Merges `addresses.<key> = <address>` into the deployment JSON, preserving every other key
/// and creating the file if it does not exist yet.
fn record_deployed_address(path: &Path, key: &str, address: Address) -> Result<(), DynError> {
    let mut deployment: serde_json::Value = match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| format!("failed to parse deployment JSON {}: {e}", path.display()))?,
        Err(_) => serde_json::json!({}),
    };

    if !deployment["addresses"].is_object() {
        deployment["addresses"] = serde_json::json!({});
    }
    deployment["addresses"][key] = serde_json::json!(format!("{address:?}"));

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create deployment JSON directory {}: {e}",
                parent.display()
            )
        })?;
    }
    let serialized = serde_json::to_string_pretty(&deployment)
        .map_err(|e| format!("failed to serialize deployment JSON: {e}"))?;
    fs::write(path, serialized)
        .map_err(|e| format!("failed to write deployment JSON {}: {e}", path.display()))?;

    println!(
        "📝 recorded addresses.{key} = {address:?} in {}",
        path.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Scenario emission
// ---------------------------------------------------------------------------------------

/// One generated scenario request.
struct ScenarioRequest {
    label: String,
    call_data: Bytes,
    verify: bool,
}

/// Renders a `run_scenario` config that drives the deployed target.
///
/// The target is referenced by the `local:<key>` sentinel rather than a literal address, so
/// the file stays correct across redeploys. `api_key` is omitted because `run_scenario`
/// already falls back to `GAS_KILLER_API_KEY`.
fn render_scenario(name: &str, router_url: &str, requests: &[ScenarioRequest]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Generated by `cargo run -p scripts --bin deploy_example -- --example {name}`.\n\
         # Overwritten on every deploy — edit scripts/examples/examples.toml instead.\n\
         #\n\
         # Run with:\n\
         #   cargo run -p scripts --bin run_scenario -- scripts/scenarios/generated/{name}.toml\n\n"
    ));
    out.push_str(&format!("router_url = {}\n", toml_string(router_url)));
    out.push_str("http_rpc   = \"$HTTP_RPC\"\n\n");
    out.push_str("[[scenarios]]\n");
    out.push_str(&format!("name = {}\n", toml_string(name)));
    out.push_str("mode = \"serial\"\n");

    for request in requests {
        out.push_str("\n  [[scenarios.requests]]\n");
        out.push_str(&format!(
            "  label          = {}\n",
            toml_string(&request.label)
        ));
        out.push_str(&format!(
            "  target_address = {}\n",
            toml_string(&format!("local:{name}"))
        ));
        out.push_str(&format!(
            "  call_data      = {}\n",
            toml_string(&format!("0x{}", alloy::hex::encode(&request.call_data)))
        ));
        out.push_str("  from_address   = \"local\"\n");
        // The router renders a payload rather than broadcasting it, so a scenario that wants
        // to observe an on-chain effect has to submit that payload itself.
        out.push_str("  submit         = true\n");
        out.push_str(&format!("  verify         = {}\n", request.verify));
    }

    out
}

/// Quotes a value as a TOML basic string.
fn toml_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

// ---------------------------------------------------------------------------------------
// On-chain steps
// ---------------------------------------------------------------------------------------

/// Rejects a signature checker that cannot actually verify signatures.
///
/// `GasKillerSDK.verifyAndUpdate` calls `checkSignatures` on whatever address the constructor
/// was given. The `BLSSigCheckOperatorStateRetriever` recorded as `blsSigCheck` in the AVS
/// deployment JSON has no such function, so a target wired to it reverts with an empty `0x`
/// at settlement time — long after deployment, with nothing pointing at the cause. A real
/// checker exposes `registryCoordinator()`; the retriever does not, which makes that getter a
/// cheap discriminator.
async fn validate_sig_checker(provider: &DynProvider, checker: Address) -> Result<(), DynError> {
    let code = provider
        .get_code_at(checker)
        .await
        .map_err(|e| format!("failed to read code at signature checker {checker:?}: {e}"))?;
    if code.is_empty() {
        return Err(format!("signature checker {checker:?} has no code deployed").into());
    }

    let selector = &keccak256("registryCoordinator()")[..4];
    let probe = TransactionRequest::default()
        .with_to(checker)
        .with_input(Bytes::copy_from_slice(selector));

    match provider.call(probe).await {
        Ok(ret) if ret.len() >= 32 => Ok(()),
        _ => Err(format!(
            "{checker:?} does not expose registryCoordinator(), so it is not a BLSSignatureChecker \
             — a target wired to it reverts with an empty 0x during verifyAndUpdate. Did you pass \
             the `{RETRIEVER_ADDRESS_KEY}` operator-state retriever? Set \
             EXAMPLE_SIG_CHECKER_ADDRESS to a real checker."
        )
        .into()),
    }
}

/// Confirms the freshly deployed target is one the router will actually settle against.
///
/// The ERC-165 check is the same gate `router::executor` applies before submitting
/// `verifyAndUpdate`, and it is the one most likely to fail on a contract built against a
/// different `solidity-sdk` revision than the service expects — the interface ID changes
/// whenever `IGasKillerSDK` does.
async fn assert_routable(
    provider: &DynProvider,
    target: Address,
    exercise_selector: Option<FixedBytes<4>>,
) -> Result<(), DynError> {
    let (interface_id, scheme) = match signature_scheme() {
        SignatureScheme::Bls => (GAS_KILLER_INTERFACE_ID, "bls"),
        SignatureScheme::Schnorr => (SCHNORR_GAS_KILLER_INTERFACE_ID, "schnorr"),
    };
    let sdk = GasKillerSDK::new(target, provider);

    let supported = sdk
        .supportsInterface(interface_id)
        .call()
        .await
        .map_err(|e| format!("supportsInterface({interface_id}) call failed on {target:?}: {e}"))?;
    if !supported {
        return Err(format!(
            "{target:?} does not report support for the {scheme} interface {interface_id}, so the \
             router will refuse to settle against it. The contract must inherit the \
             {} base contract, and its solidity-sdk revision must match the one this service \
             was built against.",
            match scheme {
                "schnorr" => "SchnorrGasKillerSDK",
                _ => "GasKillerSDK",
            }
        )
        .into());
    }

    let count = sdk
        .stateTransitionCount()
        .call()
        .await
        .map_err(|e| format!("stateTransitionCount() call failed on {target:?}: {e}"))?;

    // The digest the quorum signs is computed off-chain and must match this getter byte for
    // byte; probing it now proves the ABI is the shape the aggregation path assumes.
    if let Some(selector) = exercise_selector {
        sdk.getMessageHash(count, selector, Bytes::new())
            .call()
            .await
            .map_err(|e| format!("getMessageHash() call failed on {target:?}: {e}"))?;
    }

    println!(
        "✅ {target:?} is routable: supportsInterface({interface_id}) = true, stateTransitionCount() = {count}"
    );
    Ok(())
}

/// Sends a manifest setup call and requires it to succeed.
async fn run_setup_call(
    provider: &DynProvider,
    target: Address,
    call: &CallSpec,
    resolver: &Resolver,
) -> Result<(), DynError> {
    let args = call
        .setup_args(resolver)
        .map_err(|e| format!("setup call `{}`: {e}", call.sig))?;
    let data = encode_call(&call.sig, &args)?;
    let value = match &call.value {
        Some(raw) => parse_wei(raw)?,
        None => U256::ZERO,
    };

    let tx = TransactionRequest::default()
        .with_to(target)
        .with_value(value)
        .with_input(data);

    let receipt = provider
        .send_transaction(tx)
        .await
        .map_err(|e| format!("setup call `{}` failed to send: {e}", call.sig))?
        .get_receipt()
        .await
        .map_err(|e| format!("setup call `{}` receipt failed: {e}", call.sig))?;
    if !receipt.status() {
        return Err(format!(
            "setup call `{}` reverted (tx {:?})",
            call.sig, receipt.transaction_hash
        )
        .into());
    }
    println!("   ↳ {} (signer {})", call.sig, call.signer);
    Ok(())
}

impl CallSpec {
    fn setup_args(&self, resolver: &Resolver) -> Result<Vec<ArgValue>, DynError> {
        self.args.iter().map(|a| resolve_arg(resolver, a)).collect()
    }
}

/// Parses a wei amount given in decimal or `0x`-prefixed hex.
fn parse_wei(raw: &str) -> Result<U256, DynError> {
    let trimmed = raw.trim();
    let parsed = match trimmed.strip_prefix("0x") {
        Some(hex_body) => U256::from_str_radix(hex_body, 16),
        None => U256::from_str_radix(trimmed, 10),
    };
    parsed.map_err(|e| format!("`{raw}` is not a valid wei amount: {e}").into())
}

// ---------------------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), DynError> {
    dotenv::dotenv().ok();
    let cli = Cli::parse();

    let manifest_raw = fs::read_to_string(&cli.manifest)
        .map_err(|e| format!("failed to read manifest {}: {e}", cli.manifest.display()))?;
    let manifest: Manifest = toml::from_str(&manifest_raw)
        .map_err(|e| format!("failed to parse manifest {}: {e}", cli.manifest.display()))?;

    let selected = select_examples(&manifest, &cli.examples)?;
    let artifacts_dir = resolve_artifacts_dir(cli.artifacts.clone());
    let deploy_json = resolve_deploy_json(cli.deploy_json.clone());
    let deploy_addresses = read_deploy_addresses(&deploy_json)?;

    // Signer keys are resolved before anything else: their addresses are placeholder inputs,
    // and index 0 is the deployer.
    let signer_keys = resolve_signer_keys(&manifest.signers)?;
    let signers: Vec<PrivateKeySigner> = signer_keys
        .iter()
        .enumerate()
        .map(|(i, key)| {
            key.trim()
                .parse::<PrivateKeySigner>()
                .map_err(|_| -> DynError {
                    format!("signers[{i}] is not a valid private key").into()
                })
        })
        .collect::<Result<_, _>>()?;

    let avs = resolve_wiring_address(
        cli.avs,
        "EXAMPLE_AVS_ADDRESS",
        AVS_ADDRESS_KEY,
        &deploy_addresses,
    )?;
    let sig_checker = resolve_wiring_address(
        cli.sig_checker,
        "EXAMPLE_SIG_CHECKER_ADDRESS",
        SIG_CHECKER_ADDRESS_KEY,
        &deploy_addresses,
    )?;
    reject_retriever_as_checker(sig_checker, &deploy_addresses)?;

    let mut resolver = Resolver {
        avs,
        sig_checker,
        deploy_addresses: deploy_addresses.clone(),
        signers: signers.iter().map(|s| s.address()).collect(),
    };

    if cli.dry_run {
        println!("🧪 dry run: resolving and encoding only, no transactions will be sent");
        // A dry run validates the manifest against the artifacts, which does not require real
        // wiring — substitute a sentinel so a machine with no deployment JSON can still check
        // that every argument encodes.
        if resolver.avs.is_none() || resolver.sig_checker.is_none() {
            println!(
                "⚠️  AVS wiring unresolved; substituting {DRY_RUN_PLACEHOLDER:?} so encoding can \
                 still be validated"
            );
            resolver.avs = resolver.avs.or(Some(DRY_RUN_PLACEHOLDER));
            resolver.sig_checker = resolver.sig_checker.or(Some(DRY_RUN_PLACEHOLDER));
        }
    }
    let resolver = resolver;

    let router_url = cli
        .router_url
        .clone()
        .or_else(|| std::env::var("GAS_KILLER_ROUTER_URL").ok())
        .unwrap_or_else(|| "http://localhost:8080".to_string());

    // One wallet-backed provider per signer, so a setup call can be sent as whichever account
    // the manifest names. Index 0 is the deployer. Left empty by a dry run, which must work
    // without an RPC endpoint at all.
    let signer_providers = if cli.dry_run {
        Vec::new()
    } else {
        let http_rpc: url::Url = std::env::var("HTTP_RPC")
            .map_err(|_| "HTTP_RPC environment variable is required to deploy")?
            .parse()
            .map_err(|_| "invalid HTTP_RPC URL")?;
        if signers.is_empty() {
            return Err("the manifest must declare at least one signer".into());
        }
        println!("🔑 deployer: {:?}", signers[0].address());
        signers
            .iter()
            .map(|signer| {
                ProviderBuilder::new()
                    .wallet(EthereumWallet::from(signer.clone()))
                    .connect_http(http_rpc.clone())
                    .erased()
            })
            .collect()
    };

    if let (Some(provider), Some(checker)) = (signer_providers.first(), sig_checker) {
        validate_sig_checker(provider, checker).await?;
        println!("🔐 signature checker: {checker:?}");
    }

    for example in selected {
        println!("\n═══ {} ═══", example.name);
        deploy_one(
            example,
            &resolver,
            &signer_providers,
            &artifacts_dir,
            &deploy_json,
            &cli,
            &router_url,
        )
        .await
        .map_err(|e| format!("example `{}`: {e}", example.name))?;
    }

    println!("\n🎉 done");
    Ok(())
}

/// Deploys one example, wires it up, and emits its scenario.
///
/// `signer_providers` is one wallet-backed provider per manifest signer, empty for a dry run.
async fn deploy_one(
    example: &ExampleSpec,
    resolver: &Resolver,
    signer_providers: &[DynProvider],
    artifacts_dir: &Path,
    deploy_json: &Path,
    cli: &Cli,
    router_url: &str,
) -> Result<(), DynError> {
    let path = artifact_path(artifacts_dir, &example.artifact);
    let (abi, bytecode) = load_artifact(&path)?;
    println!("📦 artifact: {}", path.display());

    let ctor_args = example
        .ctor_args
        .iter()
        .map(|a| resolve_arg(resolver, a))
        .collect::<Result<Vec<_>, _>>()?;
    let init_code = build_init_code(&abi, &bytecode, &ctor_args)?;
    println!(
        "🧱 init code: {} bytes ({} bytes of constructor arguments)",
        init_code.len(),
        init_code.len() - bytecode.len()
    );

    // Encode the exercise calldata before deploying: a manifest typo should fail before a
    // transaction is spent, not after.
    let requests = example
        .exercise
        .iter()
        .map(|ex| {
            let args = ex
                .args
                .iter()
                .map(|a| resolve_arg(resolver, a))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ScenarioRequest {
                label: ex.label.clone().unwrap_or_else(|| example.name.clone()),
                call_data: encode_call(&ex.sig, &args)?,
                verify: ex.verify,
            })
        })
        .collect::<Result<Vec<ScenarioRequest>, DynError>>()?;

    let exercise_selector = match example.exercise.first() {
        Some(ex) => Some(parse_signature(&ex.sig)?.selector()),
        None => None,
    };

    let scenario_path = cli.scenario_dir.join(format!("{}.toml", example.name));

    let Some(provider) = signer_providers.first() else {
        for request in &requests {
            println!(
                "   ↳ {} → 0x{}",
                request.label,
                alloy::hex::encode(&request.call_data)
            );
        }
        write_scenario(
            &scenario_path,
            &render_scenario(&example.name, router_url, &requests),
        )?;
        return Ok(());
    };

    let target = match reusable_address(cli, resolver, &example.name, provider).await? {
        Some(existing) => {
            println!("♻️  reusing {existing:?} from addresses.{}", example.name);
            existing
        }
        None => {
            let receipt = provider
                .send_transaction(TransactionRequest::default().with_deploy_code(init_code))
                .await
                .map_err(|e| format!("failed to send deployment transaction: {e}"))?
                .get_receipt()
                .await
                .map_err(|e| format!("deployment transaction was not mined: {e}"))?;
            if !receipt.status() {
                return Err(format!(
                    "deployment reverted (tx {:?}) — check the constructor arguments",
                    receipt.transaction_hash
                )
                .into());
            }
            let address = receipt
                .contract_address
                .ok_or("deployment receipt carried no contract address")?;
            println!(
                "🚀 deployed at {address:?} (tx {:?})",
                receipt.transaction_hash
            );
            address
        }
    };

    assert_routable(provider, target, exercise_selector).await?;

    if !example.setup.is_empty() {
        println!("⚙️  running {} setup call(s)", example.setup.len());
        for call in &example.setup {
            let call_provider = signer_providers.get(call.signer).ok_or_else(|| {
                format!(
                    "setup call `{}` wants signer {} but the manifest declares {}",
                    call.sig,
                    call.signer,
                    signer_providers.len()
                )
            })?;
            run_setup_call(call_provider, target, call, resolver).await?;
        }
    }

    record_deployed_address(deploy_json, &example.name, target)?;
    write_scenario(
        &scenario_path,
        &render_scenario(&example.name, router_url, &requests),
    )?;

    Ok(())
}

/// Returns the already-recorded address when `--reuse` is set and it still has code.
async fn reusable_address(
    cli: &Cli,
    resolver: &Resolver,
    name: &str,
    provider: &DynProvider,
) -> Result<Option<Address>, DynError> {
    if !cli.reuse {
        return Ok(None);
    }
    let Some(recorded) = resolver.deploy_addresses.get(name) else {
        return Ok(None);
    };
    let address: Address = recorded
        .parse()
        .map_err(|_| format!("addresses.{name} is not a valid address: {recorded}"))?;
    let code = provider
        .get_code_at(address)
        .await
        .map_err(|e| format!("failed to read code at {address:?}: {e}"))?;
    Ok((!code.is_empty()).then_some(address))
}

fn write_scenario(path: &Path, contents: &str) -> Result<(), DynError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    fs::write(path, contents)
        .map_err(|e| format!("failed to write scenario {}: {e}", path.display()))?;
    println!("📄 wrote {}", path.display());
    Ok(())
}

/// Filters the manifest down to the requested examples, preserving manifest order.
fn select_examples<'a>(
    manifest: &'a Manifest,
    requested: &[String],
) -> Result<Vec<&'a ExampleSpec>, DynError> {
    if manifest.examples.is_empty() {
        return Err("the manifest declares no examples".into());
    }
    if requested.is_empty() {
        return Ok(manifest.examples.iter().collect());
    }
    for name in requested {
        if !manifest.examples.iter().any(|e| &e.name == name) {
            let known: Vec<&str> = manifest.examples.iter().map(|e| e.name.as_str()).collect();
            return Err(format!(
                "no example named `{name}`; manifest has: {}",
                known.join(", ")
            )
            .into());
        }
    }
    Ok(manifest
        .examples
        .iter()
        .filter(|e| requested.contains(&e.name))
        .collect())
}

/// Expands `$env:VAR` in the manifest's signer list, defaulting to the deployer key.
fn resolve_signer_keys(declared: &[String]) -> Result<Vec<String>, DynError> {
    let declared: Vec<String> = if declared.is_empty() {
        vec!["$env:PRIVATE_KEY".to_string()]
    } else {
        declared.to_vec()
    };
    // Only `$env:` is meaningful here — the other placeholders resolve to addresses.
    let env_only = Resolver::default();
    declared.iter().map(|k| env_only.resolve(k)).collect()
}

/// Resolves a constructor wiring address by flag, then env, then the deployment JSON.
fn resolve_wiring_address(
    flag: Option<Address>,
    env_var: &str,
    json_key: &str,
    deploy_addresses: &BTreeMap<String, String>,
) -> Result<Option<Address>, DynError> {
    if let Some(address) = flag {
        return Ok(Some(address));
    }
    if let Some(raw) = std::env::var(env_var).ok().filter(|v| !v.trim().is_empty()) {
        return Ok(Some(raw.trim().parse().map_err(|_| {
            format!("{env_var} is not a valid address: {raw}")
        })?));
    }
    match deploy_addresses.get(json_key) {
        Some(raw) => Ok(Some(raw.parse().map_err(|_| {
            format!("addresses.{json_key} is not a valid address: {raw}")
        })?)),
        None => Ok(None),
    }
}

/// Fails fast when the resolved checker is the operator-state retriever, which cannot verify
/// signatures. See [`validate_sig_checker`].
fn reject_retriever_as_checker(
    sig_checker: Option<Address>,
    deploy_addresses: &BTreeMap<String, String>,
) -> Result<(), DynError> {
    let (Some(checker), Some(retriever)) =
        (sig_checker, deploy_addresses.get(RETRIEVER_ADDRESS_KEY))
    else {
        return Ok(());
    };
    let Ok(retriever) = retriever.parse::<Address>() else {
        return Ok(());
    };
    if checker == retriever {
        return Err(format!(
            "the resolved signature checker {checker:?} is `addresses.{RETRIEVER_ADDRESS_KEY}`, the \
             BLSSigCheckOperatorStateRetriever. It has no checkSignatures, so verifyAndUpdate would \
             revert with an empty 0x. Point EXAMPLE_SIG_CHECKER_ADDRESS at a real BLSSignatureChecker."
        )
        .into());
    }
    Ok(())
}

fn resolve_artifacts_dir(flag: Option<PathBuf>) -> PathBuf {
    flag.or_else(|| {
        std::env::var("EXAMPLES_DIR")
            .ok()
            .filter(|d| !d.trim().is_empty())
            .map(|d| PathBuf::from(d).join("out"))
    })
    .unwrap_or_else(|| PathBuf::from(".examples/example-contracts/out"))
}

fn resolve_deploy_json(flag: Option<PathBuf>) -> PathBuf {
    flag.or_else(|| {
        std::env::var("AVS_DEPLOYMENT_PATH")
            .ok()
            .filter(|p| !p.trim().is_empty())
            .map(PathBuf::from)
    })
    .unwrap_or_else(|| PathBuf::from("config/.nodes/avs_deploy.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR_A: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";
    const ADDR_B: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";

    fn resolver() -> Resolver {
        Resolver {
            avs: Some(ADDR_A.parse().unwrap()),
            sig_checker: Some(ADDR_B.parse().unwrap()),
            deploy_addresses: BTreeMap::from([(
                "registryCoordinator".to_string(),
                ADDR_A.to_string(),
            )]),
            signers: vec![ADDR_A.parse().unwrap(), ADDR_B.parse().unwrap()],
        }
    }

    // ---- manifest parsing ----

    #[test]
    fn manifest_parses_scalar_and_list_arguments() {
        let manifest: Manifest = toml::from_str(
            r#"
            signers = ["$env:PRIVATE_KEY", "0xabc"]

            [[examples]]
            name = "guardedVault"
            artifact = "GuardedVault.sol:GuardedVault"
            ctor_args = ["$avs", "$sigChecker", "5000"]

              [[examples.setup]]
              sig = "deposit(uint256)"
              args = ["1000"]
              signer = 1

              [[examples.exercise]]
              label = "settle_two"
              sig = "settle(address[],int256[])"
              args = [["$signer:0", "$signer:1"], ["100", "-100"]]
            "#,
        )
        .unwrap();

        assert_eq!(manifest.signers.len(), 2);
        let example = &manifest.examples[0];
        assert_eq!(example.name, "guardedVault");
        assert_eq!(example.ctor_args.len(), 3);
        assert_eq!(example.setup[0].signer, 1);
        // verify defaults to true so a generated scenario asserts an on-chain effect.
        assert!(example.exercise[0].verify);
        assert!(matches!(example.exercise[0].args[0], ArgValue::List(ref v) if v.len() == 2));
    }

    #[test]
    fn manifest_rejects_an_unknown_field() {
        let err = toml::from_str::<Manifest>(
            r#"
            [[examples]]
            name = "x"
            artifact = "X"
            constructor_args = ["1"]
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("constructor_args"), "{err}");
    }

    // ---- placeholder resolution ----

    #[test]
    fn resolves_every_placeholder_form() {
        let r = resolver();
        assert_eq!(
            r.resolve("$avs").unwrap(),
            format!("{:?}", ADDR_A.parse::<Address>().unwrap())
        );
        assert_eq!(
            r.resolve("$sigChecker").unwrap(),
            format!("{:?}", ADDR_B.parse::<Address>().unwrap())
        );
        assert_eq!(r.resolve("$deploy:registryCoordinator").unwrap(), ADDR_A);
        assert_eq!(
            r.resolve("$signer:1").unwrap(),
            format!("{:?}", ADDR_B.parse::<Address>().unwrap())
        );
    }

    #[test]
    fn passes_through_a_literal_value() {
        assert_eq!(resolver().resolve("5000").unwrap(), "5000");
        assert_eq!(resolver().resolve(ADDR_A).unwrap(), ADDR_A);
    }

    #[test]
    fn rejects_unknown_and_out_of_range_placeholders() {
        let r = resolver();
        assert!(
            r.resolve("$nope")
                .unwrap_err()
                .to_string()
                .contains("unknown placeholder")
        );
        assert!(
            r.resolve("$signer:9")
                .unwrap_err()
                .to_string()
                .contains("out of range")
        );
        assert!(
            r.resolve("$deploy:missing")
                .unwrap_err()
                .to_string()
                .contains("not found")
        );
    }

    #[test]
    fn unresolved_wiring_names_the_ways_to_supply_it() {
        let r = Resolver::default();
        let err = r.resolve("$avs").unwrap_err().to_string();
        assert!(err.contains("--avs"), "{err}");
        assert!(err.contains("EXAMPLE_AVS_ADDRESS"), "{err}");
    }

    #[test]
    fn resolves_placeholders_inside_nested_lists() {
        let arg = ArgValue::List(vec![ArgValue::List(vec![ArgValue::Scalar(
            "$signer:0".to_string(),
        )])]);
        let resolved = resolve_arg(&resolver(), &arg).unwrap();
        let ArgValue::List(outer) = resolved else {
            panic!("expected a list");
        };
        let ArgValue::List(inner) = &outer[0] else {
            panic!("expected a nested list");
        };
        assert!(matches!(&inner[0], ArgValue::Scalar(s) if s.starts_with("0x")));
    }

    // ---- ABI coercion ----

    #[test]
    fn coerces_a_fixed_array_constructor_argument() {
        // OnchainLife's seed parameter: a uint256[16] board.
        let ty = DynSolType::FixedArray(Box::new(DynSolType::Uint(256)), 16);
        let mut items = vec![ArgValue::Scalar("0".to_string()); 16];
        items[0] = ArgValue::Scalar("2381976568446569244317409228317215686658".to_string());

        let value = coerce_arg(&ty, &ArgValue::List(items)).unwrap();
        let DynSolValue::FixedArray(words) = value else {
            panic!("expected a fixed array");
        };
        assert_eq!(words.len(), 16);
        assert_eq!(
            words[0].as_uint().unwrap().0,
            U256::from_str_radix("2381976568446569244317409228317215686658", 10).unwrap()
        );
    }

    #[test]
    fn fixed_array_length_mismatch_is_an_error() {
        let ty = DynSolType::FixedArray(Box::new(DynSolType::Uint(256)), 16);
        let err = coerce_arg(&ty, &ArgValue::List(vec![ArgValue::Scalar("0".into())]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected 16 values"), "{err}");
    }

    #[test]
    fn coerces_signed_and_address_arrays() {
        let deltas = coerce_arg(
            &DynSolType::Array(Box::new(DynSolType::Int(256))),
            &ArgValue::List(vec![
                ArgValue::Scalar("100".into()),
                ArgValue::Scalar("-100".into()),
            ]),
        )
        .unwrap();
        assert!(matches!(deltas, DynSolValue::Array(ref v) if v.len() == 2));

        let users = coerce_arg(
            &DynSolType::Array(Box::new(DynSolType::Address)),
            &ArgValue::List(vec![
                ArgValue::Scalar(ADDR_A.into()),
                ArgValue::Scalar(ADDR_B.into()),
            ]),
        )
        .unwrap();
        assert!(matches!(users, DynSolValue::Array(ref v) if v.len() == 2));
    }

    #[test]
    fn arity_mismatch_is_an_error() {
        let err = coerce_args(
            &[DynSolType::Address, DynSolType::Uint(256)],
            &[ArgValue::Scalar(ADDR_A.into())],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("expected 2 argument(s)"), "{err}");
    }

    #[test]
    fn a_bad_scalar_names_the_expected_type() {
        let err = coerce_arg(&DynSolType::Address, &ArgValue::Scalar("nope".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("address"), "{err}");
    }

    // ---- calldata encoding ----

    #[test]
    fn encodes_a_call_with_the_right_selector() {
        let data = encode_call("step(uint32)", &[ArgValue::Scalar("16".into())]).unwrap();
        // selector + one 32-byte word
        assert_eq!(data.len(), 36);
        assert_eq!(
            &data[..4],
            &keccak256("step(uint32)")[..4],
            "selector must match the signature"
        );
        assert_eq!(U256::from_be_slice(&data[4..36]), U256::from(16));
    }

    #[test]
    fn encodes_dynamic_array_arguments() {
        let data = encode_call(
            "settle(address[],int256[])",
            &[
                ArgValue::List(vec![ArgValue::Scalar(ADDR_A.into())]),
                ArgValue::List(vec![ArgValue::Scalar("0".into())]),
            ],
        )
        .unwrap();
        assert_eq!(&data[..4], &keccak256("settle(address[],int256[])")[..4]);
        // two head offsets + two (length, element) pairs
        assert_eq!(data.len(), 4 + 32 * 6);
    }

    #[test]
    fn tolerates_a_function_keyword_prefix() {
        let bare = encode_call("step(uint32)", &[ArgValue::Scalar("1".into())]).unwrap();
        let prefixed =
            encode_call("function step(uint32)", &[ArgValue::Scalar("1".into())]).unwrap();
        assert_eq!(bare, prefixed);
    }

    #[test]
    fn rejects_a_malformed_signature() {
        let err = encode_call("step(", &[]).unwrap_err().to_string();
        assert!(err.contains("not a valid function signature"), "{err}");
    }

    // ---- artifact handling ----

    #[test]
    fn artifact_paths_handle_both_spellings() {
        let dir = Path::new("out");
        assert_eq!(
            artifact_path(dir, "OnchainLife.sol:OnchainLife"),
            Path::new("out/OnchainLife.sol/OnchainLife.json")
        );
        assert_eq!(
            artifact_path(dir, "OnchainLife"),
            Path::new("out/OnchainLife.sol/OnchainLife.json")
        );
    }

    #[test]
    fn init_code_is_bytecode_plus_encoded_arguments() {
        let abi: JsonAbi = serde_json::from_str(
            r#"[{"type":"constructor","inputs":[
                 {"name":"a","type":"address"},
                 {"name":"n","type":"uint256"}
               ],"stateMutability":"nonpayable"}]"#,
        )
        .unwrap();
        let bytecode = Bytes::from(vec![0x60, 0x80]);

        let init_code = build_init_code(
            &abi,
            &bytecode,
            &[
                ArgValue::Scalar(ADDR_A.into()),
                ArgValue::Scalar("5000".into()),
            ],
        )
        .unwrap();

        assert_eq!(&init_code[..2], &[0x60, 0x80]);
        assert_eq!(init_code.len(), 2 + 64);
        assert_eq!(U256::from_be_slice(&init_code[34..66]), U256::from(5000));
    }

    #[test]
    fn init_code_rejects_arguments_for_a_constructorless_artifact() {
        let abi: JsonAbi = serde_json::from_str("[]").unwrap();
        let err = build_init_code(
            &abi,
            &Bytes::from(vec![0x00]),
            &[ArgValue::Scalar("1".into())],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no constructor"), "{err}");
    }

    // ---- deployment JSON merge ----

    #[test]
    fn recording_an_address_preserves_unrelated_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("avs_deploy.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"lastUpdate":{"block_number":"7"},"addresses":{"arraySummation":"0x1234"}}"#,
        )
        .unwrap();

        let deployed: Address = ADDR_B.parse().unwrap();
        record_deployed_address(&path, "onchainLife", deployed).unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["lastUpdate"]["block_number"], "7");
        assert_eq!(written["addresses"]["arraySummation"], "0x1234");
        assert_eq!(written["addresses"]["onchainLife"], format!("{deployed:?}"));
    }

    #[test]
    fn recording_an_address_creates_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.json");
        let deployed: Address = ADDR_A.parse().unwrap();

        record_deployed_address(&path, "guardedVault", deployed).unwrap();

        let addresses = read_deploy_addresses(&path).unwrap();
        assert_eq!(addresses["guardedVault"], format!("{deployed:?}"));
    }

    #[test]
    fn reading_addresses_tolerates_a_missing_file() {
        let addresses = read_deploy_addresses(Path::new("/nonexistent/avs_deploy.json")).unwrap();
        assert!(addresses.is_empty());
    }

    // ---- scenario emission ----

    #[test]
    fn scenario_targets_the_local_sentinel_and_enables_submit() {
        let rendered = render_scenario(
            "onchainLife",
            "http://localhost:8080",
            &[ScenarioRequest {
                label: "life_step_1".to_string(),
                call_data: Bytes::from(vec![0xde, 0xad, 0xbe, 0xef]),
                verify: true,
            }],
        );

        assert!(rendered.contains(r#"target_address = "local:onchainLife""#));
        assert!(rendered.contains(r#"from_address   = "local""#));
        assert!(rendered.contains(r#"call_data      = "0xdeadbeef""#));
        assert!(rendered.contains("submit         = true"));
        assert!(rendered.contains("verify         = true"));
        assert!(rendered.contains(r#"http_rpc   = "$HTTP_RPC""#));
        // api_key is intentionally absent: run_scenario falls back to GAS_KILLER_API_KEY.
        assert!(!rendered.contains("api_key"));
    }

    #[test]
    fn generated_scenario_round_trips_through_the_toml_parser() {
        let rendered = render_scenario(
            "guardedVault",
            "https://testnet.gaskiller.xyz",
            &[
                ScenarioRequest {
                    label: "settle_two".to_string(),
                    call_data: Bytes::from(vec![0x01, 0x02, 0x03, 0x04]),
                    verify: true,
                },
                ScenarioRequest {
                    label: "settle_again".to_string(),
                    call_data: Bytes::from(vec![0x05, 0x06, 0x07, 0x08]),
                    verify: false,
                },
            ],
        );

        let parsed: toml::Value = toml::from_str(&rendered).unwrap();
        assert_eq!(
            parsed["router_url"].as_str(),
            Some("https://testnet.gaskiller.xyz")
        );
        let requests = parsed["scenarios"][0]["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1]["verify"].as_bool(), Some(false));
    }

    #[test]
    fn toml_strings_escape_quotes_and_backslashes() {
        assert_eq!(toml_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    // ---- selection and wiring resolution ----

    #[test]
    fn selecting_examples_filters_and_reports_unknown_names() {
        let manifest: Manifest = toml::from_str(
            r#"
            [[examples]]
            name = "a"
            artifact = "A"

            [[examples]]
            name = "b"
            artifact = "B"
            "#,
        )
        .unwrap();

        // No selection means everything, in manifest order.
        let all = select_examples(&manifest, &[]).unwrap();
        assert_eq!(
            all.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );

        let one = select_examples(&manifest, &["b".to_string()]).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].name, "b");

        let err = select_examples(&manifest, &["c".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("no example named `c`"), "{err}");
        assert!(err.contains("a, b"), "{err}");
    }

    #[test]
    fn flag_beats_env_and_json_for_wiring() {
        let json = BTreeMap::from([(AVS_ADDRESS_KEY.to_string(), ADDR_B.to_string())]);
        let flag: Address = ADDR_A.parse().unwrap();
        let resolved = resolve_wiring_address(
            Some(flag),
            "EXAMPLE_AVS_ADDRESS_UNSET_IN_TEST",
            AVS_ADDRESS_KEY,
            &json,
        )
        .unwrap();
        assert_eq!(resolved, Some(flag));
    }

    #[test]
    fn wiring_falls_back_to_the_deployment_json() {
        let json = BTreeMap::from([(AVS_ADDRESS_KEY.to_string(), ADDR_B.to_string())]);
        let resolved = resolve_wiring_address(
            None,
            "EXAMPLE_AVS_ADDRESS_UNSET_IN_TEST",
            AVS_ADDRESS_KEY,
            &json,
        )
        .unwrap();
        assert_eq!(resolved, Some(ADDR_B.parse().unwrap()));
    }

    #[test]
    fn missing_wiring_resolves_to_none_rather_than_erroring() {
        let resolved = resolve_wiring_address(
            None,
            "EXAMPLE_AVS_ADDRESS_UNSET_IN_TEST",
            AVS_ADDRESS_KEY,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(resolved, None);
    }

    #[test]
    fn the_operator_state_retriever_is_refused_as_a_checker() {
        let json = BTreeMap::from([(RETRIEVER_ADDRESS_KEY.to_string(), ADDR_A.to_string())]);
        let err = reject_retriever_as_checker(Some(ADDR_A.parse().unwrap()), &json)
            .unwrap_err()
            .to_string();
        assert!(err.contains("checkSignatures"), "{err}");

        // A different checker is fine.
        reject_retriever_as_checker(Some(ADDR_B.parse().unwrap()), &json).unwrap();
    }

    // ---- misc ----

    #[test]
    fn signer_list_defaults_to_the_deployer_key() {
        // SAFETY: single-threaded test setting a variable only this test reads.
        unsafe { std::env::set_var("PRIVATE_KEY", "0xdeadbeef") };
        assert_eq!(resolve_signer_keys(&[]).unwrap(), vec!["0xdeadbeef"]);
    }

    #[test]
    fn wei_parses_decimal_and_hex() {
        assert_eq!(parse_wei("1000").unwrap(), U256::from(1000));
        assert_eq!(parse_wei("0x10").unwrap(), U256::from(16));
        assert!(parse_wei("twelve").is_err());
    }

    #[test]
    fn artifacts_dir_defaults_under_the_examples_checkout() {
        assert_eq!(
            resolve_artifacts_dir(Some(PathBuf::from("custom/out"))),
            PathBuf::from("custom/out")
        );
    }
}
