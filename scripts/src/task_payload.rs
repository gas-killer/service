//! Client-side half of the ingress task lifecycle, shared by the script binaries.
//!
//! With `INGRESS=true` the router *renders* a `verifyAndUpdate` transaction rather than
//! broadcasting it: the quorum signs, the executor persists a payload, and the caller is the
//! one who signs and sends it. So confirming that a task actually settled on-chain takes
//! three steps, not one — poll `GET /tasks/{id}` until the payload is rendered, submit it,
//! and only then does the target's `stateTransitionCount()` move.
//!
//! A rendered payload is only valid for a window (`valid_until_block`), so the poll and the
//! submission belong together and are kept in one module.

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use gas_killer_common::PayloadView;
use url::Url;

type DynError = Box<dyn std::error::Error + Send + Sync>;

/// How long to wait for a task to reach `ready` before giving up.
pub const DEFAULT_READY_TIMEOUT_SECS: u64 = 150;

/// How often to re-poll a task's status while waiting for it to become `ready`.
pub const READY_POLL_INTERVAL_SECS: u64 = 5;

/// Builds the `GET /tasks/{task_id}` URL for a router reachable at `base_url`.
///
/// `base_url` may be either the router root (`http://host:8080`) or a submission endpoint
/// (`http://host:8080/tasks`); the path is replaced either way.
pub fn task_status_url(base_url: &str, task_id: &str) -> Result<Url, DynError> {
    let mut url =
        Url::parse(base_url).map_err(|e| format!("invalid router URL {base_url}: {e}"))?;
    url.set_path(&format!("/tasks/{task_id}"));
    Ok(url)
}

/// Reads the submitter key, preferring `FUNDED_KEY` over `PRIVATE_KEY`.
///
/// The local stack runs against an Anvil fork whose dev accounts are funded, so a
/// hand-edited `.env` carrying a real-but-unfunded `PRIVATE_KEY` should not break local
/// submission.
pub fn submitter_key() -> Result<String, DynError> {
    std::env::var("FUNDED_KEY")
        .or_else(|_| std::env::var("PRIVATE_KEY"))
        .map_err(|_| "FUNDED_KEY or PRIVATE_KEY required to submit the payload".into())
}

/// Polls `GET /tasks/{id}` until the task is `ready` and returns its rendered payload.
///
/// A `ready` response carries the transaction request the user submits as-is. A `failed`/`expired`
/// settlement, or a non-success status (e.g. `409 PAYLOAD_EXPIRED` if the payload went stale before
/// we submitted), is terminal and surfaces as an error.
pub async fn wait_for_ready_payload(
    client: &reqwest::Client,
    task_status_url: &Url,
    api_key: Option<&str>,
    task_id: &str,
    timeout_secs: u64,
) -> Result<PayloadView, DynError> {
    use tokio::time::{Duration, Instant, sleep};
    let max_wait_time = Duration::from_secs(timeout_secs);
    let check_interval = Duration::from_secs(READY_POLL_INTERVAL_SECS);
    let start_time = Instant::now();

    loop {
        let mut req = client.get(task_status_url.clone());
        if let Some(api_key) = api_key {
            req = req.header("Authorization", format!("Bearer {api_key}"));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("Failed to poll task {}: {}", task_id, e))?;
        let status_code = resp.status();
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse task {} response: {}", task_id, e))?;

        // A non-success status (e.g. 409 PAYLOAD_EXPIRED) carries an error envelope, not a task.
        if !status_code.is_success() {
            let code = body
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(|c| c.as_str())
                .unwrap_or("UNKNOWN");
            return Err(
                format!("Task {task_id} status query returned {status_code} ({code})").into(),
            );
        }

        let task_status = body
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string();

        println!(
            "task {}: status={}, elapsed={:.1}s",
            task_id,
            task_status,
            start_time.elapsed().as_secs_f64()
        );

        match task_status.as_str() {
            "ready" => {
                let payload_value = body
                    .get("payload")
                    .cloned()
                    .ok_or_else(|| format!("ready task {task_id} carried no payload"))?;
                let payload: PayloadView = serde_json::from_value(payload_value)
                    .map_err(|e| format!("failed to parse task {task_id} payload: {e}"))?;
                println!(
                    "✅ task {} ready: to={:?} chain_id={} estimated_gas={} valid_until_block={}",
                    task_id,
                    payload.to,
                    payload.chain_id,
                    payload.estimated_gas,
                    payload.valid_until_block
                );
                return Ok(payload);
            }
            "failed" | "expired" => {
                let error = body
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("<no error recorded>");
                return Err(
                    format!("Task {} settled as '{}': {}", task_id, task_status, error).into(),
                );
            }
            _ => {}
        }

        if start_time.elapsed() >= max_wait_time {
            return Err(format!(
                "Task {} did not reach 'ready' within {:.0}s (last status: {})",
                task_id,
                max_wait_time.as_secs_f64(),
                task_status
            )
            .into());
        }

        sleep(check_interval).await;
    }
}

/// Signs and submits a rendered payload with a funded key, mirroring what an integrator does, and
/// asserts the `verifyAndUpdate` transaction lands successfully.
pub async fn submit_payload(
    payload: &PayloadView,
    http_rpc: &str,
    private_key: &str,
) -> Result<(), DynError> {
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|_| "invalid FUNDED_KEY/PRIVATE_KEY format")?;
    let sender = signer.address();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(signer))
        .connect_http(http_rpc.parse().map_err(|_| "invalid HTTP_RPC URL")?);

    let tx = TransactionRequest::default()
        .with_to(payload.to)
        .with_value(payload.value)
        .with_input(payload.data.clone());

    println!(
        "Submitting verifyAndUpdate as {sender} to {:?} ({} bytes calldata)",
        payload.to,
        payload.data.len()
    );
    let pending = provider
        .send_transaction(tx)
        .await
        .map_err(|e| format!("failed to send verifyAndUpdate: {e}"))?;
    let receipt = pending
        .get_receipt()
        .await
        .map_err(|e| format!("failed to get verifyAndUpdate receipt: {e}"))?;
    if !receipt.status() {
        return Err(format!(
            "verifyAndUpdate reverted (tx {:?}, block {:?})",
            receipt.transaction_hash, receipt.block_number
        )
        .into());
    }
    println!(
        "✅ verifyAndUpdate landed: tx {:?} in block {:?}",
        receipt.transaction_hash, receipt.block_number
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_url_from_router_root() {
        let url = task_status_url("http://localhost:8080", "abc123").unwrap();
        assert_eq!(url.as_str(), "http://localhost:8080/tasks/abc123");
    }

    #[test]
    fn status_url_replaces_an_existing_submission_path() {
        let url = task_status_url("http://localhost:8080/tasks", "abc123").unwrap();
        assert_eq!(url.as_str(), "http://localhost:8080/tasks/abc123");
    }

    #[test]
    fn status_url_preserves_host_and_scheme() {
        let url = task_status_url("https://testnet.gaskiller.xyz", "t-1").unwrap();
        assert_eq!(url.as_str(), "https://testnet.gaskiller.xyz/tasks/t-1");
    }

    #[test]
    fn status_url_rejects_a_non_url() {
        assert!(task_status_url("not a url", "abc").is_err());
    }
}
