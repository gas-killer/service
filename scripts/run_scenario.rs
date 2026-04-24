use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder};
use gas_killer_common::ReadOnlyProvider;
use gas_killer_common::bindings::gaskillersdk::GasKillerSDK;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use url::Url;

// ── Config types ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default = "default_router_url")]
    router_url: String,
    /// Required when any request uses `block_height = 0`, `verify = true`, or `transition_index = "auto"`.
    http_rpc: Option<String>,
    #[serde(default)]
    scenarios: Vec<Scenario>,
}

fn default_router_url() -> String {
    "http://localhost:8080".to_string()
}

#[derive(Debug, Clone, Default)]
enum TransitionIndex {
    #[default]
    Auto,
    Fixed(u64),
}

impl<'de> serde::Deserialize<'de> for TransitionIndex {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = TransitionIndex;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, r#"an integer or "auto""#)
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(TransitionIndex::Fixed(v))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                u64::try_from(v)
                    .map(TransitionIndex::Fixed)
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Signed(v), &self))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if v == "auto" {
                    Ok(TransitionIndex::Auto)
                } else {
                    Err(E::invalid_value(serde::de::Unexpected::Str(v), &self))
                }
            }
        }
        d.deserialize_any(Visitor)
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
enum Mode {
    #[default]
    Serial,
    Parallel,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Serial => write!(f, "serial"),
            Mode::Parallel => write!(f, "parallel"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    #[serde(default)]
    mode: Mode,
    /// Milliseconds to wait between requests in serial mode (ignored for parallel).
    #[serde(default)]
    delay_between_ms: u64,
    requests: Vec<RequestConfig>,
}

#[derive(Debug, Deserialize, Clone)]
struct RequestConfig {
    /// Human-readable label for output. Defaults to "request N".
    label: Option<String>,
    target_address: String,
    /// ABI-encoded call data as a 0x-prefixed hex string.
    call_data: String,
    from_address: String,
    /// State transition sequence number. Set to "auto" or omit to fetch stateTransitionCount() from the contract (requires `http_rpc`).
    #[serde(default)]
    transition_index: TransitionIndex,
    /// Wei value as a decimal or 0x-prefixed hex string (default: "0").
    #[serde(default = "default_value")]
    value: String,
    /// Set to 0 to auto-fetch the current block from `http_rpc`.
    #[serde(default)]
    block_height: u64,
    /// When true, poll stateTransitionCount() after a 200 to confirm verifyAndUpdate ran.
    #[serde(default)]
    verify: bool,
    /// How long to wait for on-chain confirmation (seconds, default: 150).
    #[serde(default = "default_verify_timeout")]
    verify_timeout_secs: u64,
}

fn default_value() -> String {
    "0".to_string()
}

fn default_verify_timeout() -> u64 {
    150
}

// ── API wire types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ApiRequestBody {
    target_address: String,
    call_data: Vec<u8>,
    transition_index: u64,
    from_address: String,
    value: String,
    block_height: u64,
}

#[derive(Debug, Serialize)]
struct ApiRequest {
    body: ApiRequestBody,
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    success: bool,
    message: String,
}

// ── Result types ──────────────────────────────────────────────────────────────

enum OnChainResult {
    Confirmed { elapsed: Duration },
    TimedOut { elapsed: Duration },
    Error(String),
}

struct RequestResult {
    label: String,
    status: u16,
    api_success: bool,
    message: String,
    elapsed: Duration,
    on_chain: Option<OnChainResult>,
}

impl RequestResult {
    fn passed(&self) -> bool {
        if !self.api_success || self.status != 200 {
            return false;
        }
        match &self.on_chain {
            None => true,
            Some(OnChainResult::Confirmed { .. }) => true,
            Some(OnChainResult::TimedOut { .. }) | Some(OnChainResult::Error(_)) => false,
        }
    }
}

// ── On-chain verification ─────────────────────────────────────────────────────

async fn verify_on_chain(
    provider: &ReadOnlyProvider,
    target: Address,
    initial_count: u64,
    timeout: Duration,
) -> OnChainResult {
    let contract = GasKillerSDK::new(target, provider.clone());
    let poll_interval = Duration::from_secs(10);
    let start = Instant::now();

    loop {
        match contract.stateTransitionCount().call().await {
            Ok(count) => {
                let count = count.to::<u64>();
                println!(
                    "       stateTransitionCount: {} (initial: {}, elapsed: {:.0}s)",
                    count,
                    initial_count,
                    start.elapsed().as_secs_f64(),
                );
                if count > initial_count {
                    return OnChainResult::Confirmed {
                        elapsed: start.elapsed(),
                    };
                }
            }
            Err(e) => {
                return OnChainResult::Error(format!("stateTransitionCount call failed: {e}"));
            }
        }

        if start.elapsed() >= timeout {
            return OnChainResult::TimedOut {
                elapsed: start.elapsed(),
            };
        }

        sleep(poll_interval).await;
    }
}

// ── Core execution ────────────────────────────────────────────────────────────

async fn send_request(
    client: &Client,
    router_url: &str,
    cfg: &RequestConfig,
    current_block: u64,
    provider: Option<&ReadOnlyProvider>,
    index: usize,
) -> RequestResult {
    let label = cfg
        .label
        .clone()
        .unwrap_or_else(|| format!("request {}", index + 1));

    let block_height = if cfg.block_height == 0 {
        current_block
    } else {
        cfg.block_height
    };

    let call_data = match parse_hex_bytes(&cfg.call_data) {
        Ok(b) => b,
        Err(e) => {
            return RequestResult {
                label,
                status: 0,
                api_success: false,
                message: format!("invalid call_data hex: {e}"),
                elapsed: Duration::ZERO,
                on_chain: None,
            };
        }
    };

    let needs_count = matches!(cfg.transition_index, TransitionIndex::Auto) || cfg.verify;

    // Fetch stateTransitionCount once — used for auto transition_index and/or verify baseline.
    let on_chain_count: Option<u64> = if needs_count {
        let addr = match cfg.target_address.parse::<Address>() {
            Ok(a) => a,
            Err(e) => {
                return RequestResult {
                    label,
                    status: 0,
                    api_success: false,
                    message: format!("invalid target_address: {e}"),
                    elapsed: Duration::ZERO,
                    on_chain: None,
                };
            }
        };
        match provider {
            Some(p) => match GasKillerSDK::new(addr, p.clone())
                .stateTransitionCount()
                .call()
                .await
            {
                Ok(v) => Some(v.to::<u64>()),
                Err(e) => {
                    return RequestResult {
                        label,
                        status: 0,
                        api_success: false,
                        message: format!("failed to read stateTransitionCount: {e}"),
                        elapsed: Duration::ZERO,
                        on_chain: None,
                    };
                }
            },
            None => {
                let reason = if matches!(cfg.transition_index, TransitionIndex::Auto) {
                    "`http_rpc` is required when `transition_index = \"auto\"`"
                } else {
                    "`http_rpc` is required when `verify = true`"
                };
                return RequestResult {
                    label,
                    status: 0,
                    api_success: false,
                    message: reason.to_string(),
                    elapsed: Duration::ZERO,
                    on_chain: None,
                };
            }
        }
    } else {
        None
    };

    let transition_index = match cfg.transition_index {
        TransitionIndex::Fixed(n) => n,
        TransitionIndex::Auto => on_chain_count.unwrap(),
    };

    let payload = ApiRequest {
        body: ApiRequestBody {
            target_address: cfg.target_address.clone(),
            call_data,
            transition_index,
            from_address: cfg.from_address.clone(),
            value: cfg.value.clone(),
            block_height,
        },
    };

    let url = format!("{}/trigger", router_url.trim_end_matches('/'));
    let start = Instant::now();
    let mut req = client.post(&url).json(&payload);
    if let Ok(password) = std::env::var("INGRESS_PASSWORD") {
        if !password.is_empty() {
            req = req.header("Authorization", format!("Bearer {password}"));
        }
    }
    let resp = req.send().await;
    let elapsed = start.elapsed();

    let (status, api_success, message) = match resp {
        Err(e) => (0u16, false, format!("connection error: {e}")),
        Ok(r) => {
            let status = r.status().as_u16();
            match r.text().await {
                Ok(body_text) => match serde_json::from_str::<ApiResponse>(&body_text) {
                    Ok(body) => (status, body.success, body.message),
                    Err(e) => {
                        let trimmed = body_text.trim();
                        let msg = if trimmed.is_empty() {
                            format!("non-ApiResponse body (HTTP {status}); parse error: {e}")
                        } else {
                            format!("{trimmed} (non-ApiResponse: {e})")
                        };
                        (status, false, msg)
                    }
                },
                Err(e) => (status, false, format!("failed to read response body: {e}")),
            }
        }
    };

    // Verify on-chain only if the API accepted the request
    let on_chain = if cfg.verify && api_success && status == 200 {
        if let (Some(p), Some(count), Ok(addr)) = (
            provider,
            on_chain_count,
            cfg.target_address.parse::<Address>(),
        ) {
            println!("       Waiting for on-chain confirmation...");
            Some(
                verify_on_chain(p, addr, count, Duration::from_secs(cfg.verify_timeout_secs)).await,
            )
        } else {
            None
        }
    } else {
        None
    };

    RequestResult {
        label,
        status,
        api_success,
        message,
        elapsed,
        on_chain,
    }
}

fn print_result(result: &RequestResult, index: usize, total: usize) {
    let icon = if result.passed() { "✅" } else { "❌" };
    let status_str = if result.status == 0 {
        "—".to_string()
    } else {
        result.status.to_string()
    };
    println!(
        "  [{}/{}] {}  {} ({})  {:.0}ms  \"{}\"",
        index + 1,
        total,
        icon,
        result.label,
        status_str,
        result.elapsed.as_millis(),
        result.message,
    );
    match &result.on_chain {
        Some(OnChainResult::Confirmed { elapsed }) => {
            println!(
                "         ⛓️  verifyAndUpdate confirmed  ({:.0}s)",
                elapsed.as_secs_f64()
            );
        }
        Some(OnChainResult::TimedOut { elapsed }) => {
            println!(
                "         ⛓️  ❌ verifyAndUpdate not seen after {:.0}s",
                elapsed.as_secs_f64()
            );
        }
        Some(OnChainResult::Error(e)) => {
            println!("         ⛓️  ❌ on-chain check failed: {e}");
        }
        None => {}
    }
}

async fn run_scenario(
    client: Arc<Client>,
    router_url: Arc<String>,
    scenario: &Scenario,
    current_block: u64,
    provider: Option<Arc<ReadOnlyProvider>>,
) -> Vec<RequestResult> {
    let total = scenario.requests.len();
    let delay_str = if scenario.mode == Mode::Serial && scenario.delay_between_ms > 0 {
        format!(", {}ms delay", scenario.delay_between_ms)
    } else {
        String::new()
    };
    println!(
        "\n━━━  {}  [{}{}, {} requests]  ━━━",
        scenario.name, scenario.mode, delay_str, total,
    );

    match scenario.mode {
        Mode::Serial => {
            let mut results = Vec::with_capacity(total);
            for (i, req_cfg) in scenario.requests.iter().enumerate() {
                let result = send_request(
                    &client,
                    &router_url,
                    req_cfg,
                    current_block,
                    provider.as_deref(),
                    i,
                )
                .await;
                print_result(&result, i, total);
                results.push(result);
                if i + 1 < total && scenario.delay_between_ms > 0 {
                    sleep(Duration::from_millis(scenario.delay_between_ms)).await;
                }
            }
            results
        }
        Mode::Parallel => {
            let handles: Vec<_> = scenario
                .requests
                .iter()
                .enumerate()
                .map(|(i, req_cfg)| {
                    let client = client.clone();
                    let router_url = router_url.clone();
                    let req_cfg = req_cfg.clone();
                    let provider = provider.clone();
                    tokio::spawn(async move {
                        send_request(
                            &client,
                            &router_url,
                            &req_cfg,
                            current_block,
                            provider.as_deref(),
                            i,
                        )
                        .await
                    })
                })
                .collect();

            let mut results = Vec::with_capacity(total);
            for handle in handles {
                results.push(handle.await.expect("task panicked"));
            }
            for (i, result) in results.iter().enumerate() {
                print_result(result, i, total);
            }
            results
        }
    }
}

fn print_scenario_summary(results: &[RequestResult]) {
    let passed = results.iter().filter(|r| r.passed()).count();
    let total = results.len();
    let verified = results
        .iter()
        .filter(|r| matches!(&r.on_chain, Some(OnChainResult::Confirmed { .. })))
        .count();
    let verify_requested = results.iter().filter(|r| r.on_chain.is_some()).count();

    if passed == total {
        if verify_requested > 0 {
            println!(
                "  Summary: {}/{} passed ✅  ({}/{} on-chain verified)",
                passed, total, verified, verify_requested
            );
        } else {
            println!("  Summary: {}/{} passed ✅", passed, total);
        }
    } else if verify_requested > 0 {
        println!(
            "  Summary: {}/{} passed ❌  ({}/{} on-chain verified)",
            passed, total, verified, verify_requested
        );
    } else {
        println!("  Summary: {}/{} passed ❌", passed, total);
    }
}

fn parse_hex_bytes(s: &str) -> Result<Vec<u8>, alloy::hex::FromHexError> {
    let stripped = s.trim_start_matches("0x").trim_start_matches("0X");
    alloy::hex::decode(stripped)
}

/// Replace `$VAR_NAME` tokens in `s` with values from the environment.
fn interpolate_env(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let mut name = String::new();
        while let Some(&nc) = chars.peek() {
            if nc.is_ascii_alphanumeric() || nc == '_' {
                name.push(nc);
                chars.next();
            } else {
                break;
            }
        }
        if name.is_empty() {
            out.push('$');
        } else {
            match std::env::var(&name) {
                Ok(val) => out.push_str(&val),
                Err(_) => return Err(format!("env var ${name} is not set")),
            }
        }
    }
    Ok(out)
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: run_scenario <config.toml> [--scenarios <name1,name2,...>]");
        eprintln!(
            "Example: cargo run -p scripts --bin run_scenario -- scripts/scenarios/example.toml"
        );
        eprintln!(
            "Example: cargo run -p scripts --bin run_scenario -- scripts/scenarios/example.toml --scenarios smoke,stress"
        );
        std::process::exit(1);
    }

    dotenv::dotenv().ok();

    let config_path = &args[1];
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| format!("failed to read {config_path}: {e}"))?;
    let config_str = interpolate_env(&raw).map_err(|e| format!("config error: {e}"))?;
    let mut config: Config =
        toml::from_str(&config_str).map_err(|e| format!("failed to parse config: {e}"))?;

    if let Some(pos) = args.iter().position(|a| a == "--scenarios") {
        let names: Vec<&str> = args
            .get(pos + 1)
            .ok_or("--scenarios requires a comma-separated list of names")?
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        config
            .scenarios
            .retain(|s| names.contains(&s.name.as_str()));
        if config.scenarios.is_empty() {
            eprintln!("No scenarios matched: {}", names.join(", "));
            std::process::exit(1);
        }
    }

    if config.scenarios.is_empty() {
        eprintln!("No scenarios defined in config.");
        std::process::exit(1);
    }

    let needs_rpc = config
        .scenarios
        .iter()
        .flat_map(|s| s.requests.iter())
        .any(|r| {
            r.block_height == 0 || r.verify || matches!(r.transition_index, TransitionIndex::Auto)
        });

    let provider: Option<Arc<ReadOnlyProvider>> = if needs_rpc {
        let rpc = config
            .http_rpc
            .as_deref()
            .ok_or("`http_rpc` is required when block_height = 0, verify = true, or transition_index = \"auto\"")?;
        Some(Arc::new(
            ProviderBuilder::new().connect_http(Url::parse(rpc)?),
        ))
    } else {
        None
    };

    let current_block = if config
        .scenarios
        .iter()
        .flat_map(|s| s.requests.iter())
        .any(|r| r.block_height == 0)
    {
        let block = provider.as_ref().unwrap().get_block_number().await?;
        println!("Current block: {block}");
        block
    } else {
        0
    };

    let client = Arc::new(Client::builder().timeout(Duration::from_secs(30)).build()?);
    let router_url = Arc::new(config.router_url.clone());

    println!("Router: {}", config.router_url);

    let mut all_results: Vec<RequestResult> = Vec::new();

    for scenario in &config.scenarios {
        let results = run_scenario(
            client.clone(),
            router_url.clone(),
            scenario,
            current_block,
            provider.clone(),
        )
        .await;
        print_scenario_summary(&results);
        all_results.extend(results);
    }

    let total = all_results.len();
    let passed = all_results.iter().filter(|r| r.passed()).count();
    let failed = total - passed;

    println!("\n━━━  Overall: {}/{} passed", passed, total);
    if failed > 0 {
        println!("     Failed requests:");
        for r in all_results.iter().filter(|r| !r.passed()) {
            println!("       ❌ {}  ({})  \"{}\"", r.label, r.status, r.message);
        }
        std::process::exit(1);
    }

    Ok(())
}
