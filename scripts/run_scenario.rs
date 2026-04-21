use alloy::providers::{Provider, ProviderBuilder};
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
    /// Required when any request uses `block_height = 0` (auto-fetch).
    http_rpc: Option<String>,
    #[serde(default)]
    scenarios: Vec<Scenario>,
}

fn default_router_url() -> String {
    "http://localhost:8080".to_string()
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Serial,
    Parallel,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Serial
    }
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
    #[serde(default)]
    transition_index: u64,
    /// Wei value as a decimal or 0x-prefixed hex string (default: "0").
    #[serde(default = "default_value")]
    value: String,
    /// Set to 0 to auto-fetch the current block from `http_rpc`.
    #[serde(default)]
    block_height: u64,
}

fn default_value() -> String {
    "0".to_string()
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

struct RequestResult {
    label: String,
    status: u16,
    api_success: bool,
    message: String,
    elapsed: Duration,
}

impl RequestResult {
    fn passed(&self) -> bool {
        self.api_success && self.status == 200
    }
}

// ── Core execution ────────────────────────────────────────────────────────────

async fn send_request(
    client: &Client,
    router_url: &str,
    cfg: &RequestConfig,
    current_block: u64,
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
            };
        }
    };

    let payload = ApiRequest {
        body: ApiRequestBody {
            target_address: cfg.target_address.clone(),
            call_data,
            transition_index: cfg.transition_index,
            from_address: cfg.from_address.clone(),
            value: cfg.value.clone(),
            block_height,
        },
    };

    let url = format!("{}/trigger", router_url.trim_end_matches('/'));
    let start = Instant::now();
    let resp = client.post(&url).json(&payload).send().await;
    let elapsed = start.elapsed();

    match resp {
        Err(e) => RequestResult {
            label,
            status: 0,
            api_success: false,
            message: format!("connection error: {e}"),
            elapsed,
        },
        Ok(r) => {
            let status = r.status().as_u16();
            match r.json::<ApiResponse>().await {
                Ok(body) => RequestResult {
                    label,
                    status,
                    api_success: body.success,
                    message: body.message,
                    elapsed,
                },
                Err(e) => RequestResult {
                    label,
                    status,
                    api_success: false,
                    message: format!("failed to parse response: {e}"),
                    elapsed,
                },
            }
        }
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
}

async fn run_scenario(
    client: Arc<Client>,
    router_url: Arc<String>,
    scenario: &Scenario,
    current_block: u64,
) -> Vec<RequestResult> {
    let total = scenario.requests.len();
    println!(
        "\n━━━  {}  [{}{}{}]  ━━━",
        scenario.name,
        scenario.mode,
        if scenario.mode == Mode::Serial && scenario.delay_between_ms > 0 {
            format!(", {}ms delay", scenario.delay_between_ms)
        } else {
            String::new()
        },
        format!(", {} requests", total),
    );

    match scenario.mode {
        Mode::Serial => {
            let mut results = Vec::with_capacity(total);
            for (i, req_cfg) in scenario.requests.iter().enumerate() {
                let result = send_request(&client, &router_url, req_cfg, current_block, i).await;
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
                    tokio::spawn(async move {
                        send_request(&client, &router_url, &req_cfg, current_block, i).await
                    })
                })
                .collect();

            let mut results = Vec::with_capacity(total);
            for handle in handles {
                results.push(handle.await.expect("task panicked"));
            }
            // Sort by original index so output is deterministic
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
    if passed == total {
        println!("  Summary: {}/{} passed ✅", passed, total);
    } else {
        println!("  Summary: {}/{} passed ❌", passed, total);
    }
}

fn parse_hex_bytes(s: &str) -> Result<Vec<u8>, alloy::hex::FromHexError> {
    let stripped = s.trim_start_matches("0x").trim_start_matches("0X");
    alloy::hex::decode(stripped)
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: diagnose <config.toml>");
        eprintln!("Example: cargo run -p scripts --bin diagnose -- scripts/diagnose_example.toml");
        std::process::exit(1);
    }

    let config_path = &args[1];
    let config_str = std::fs::read_to_string(config_path)
        .map_err(|e| format!("failed to read {config_path}: {e}"))?;
    let config: Config =
        toml::from_str(&config_str).map_err(|e| format!("failed to parse config: {e}"))?;

    if config.scenarios.is_empty() {
        eprintln!("No scenarios defined in config.");
        std::process::exit(1);
    }

    // Resolve current block once if any request needs it
    let needs_block = config
        .scenarios
        .iter()
        .flat_map(|s| s.requests.iter())
        .any(|r| r.block_height == 0);

    let current_block = if needs_block {
        let rpc = config
            .http_rpc
            .as_deref()
            .ok_or("`http_rpc` is required when block_height = 0")?;
        let provider = ProviderBuilder::new().connect_http(Url::parse(rpc)?);
        let block = provider.get_block_number().await?;
        println!("Current block: {block}");
        block
    } else {
        0
    };

    let client = Arc::new(
        Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?,
    );
    let router_url = Arc::new(config.router_url.clone());

    println!("Router: {}", config.router_url);

    let mut all_results: Vec<RequestResult> = Vec::new();

    for scenario in &config.scenarios {
        let results =
            run_scenario(client.clone(), router_url.clone(), scenario, current_block).await;
        print_scenario_summary(&results);
        all_results.extend(results);
    }

    // Overall summary
    let total = all_results.len();
    let passed = all_results.iter().filter(|r| r.passed()).count();
    let failed = total - passed;

    println!("\n━━━  Overall: {}/{} passed", passed, total);
    if failed > 0 {
        println!("     Failed requests:");
        for r in all_results.iter().filter(|r| !r.passed()) {
            println!(
                "       ❌ {}  ({})  \"{}\"",
                r.label, r.status, r.message
            );
        }
        std::process::exit(1);
    }

    Ok(())
}
