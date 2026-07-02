//! Operator tool: mint a new API key against a deployed router's admin API.
//!
//! Targets an environment by name (`--env`) or an explicit base URL (`--url`), calling
//! `POST /admin/keys` on the router. Authentication uses the `ADMIN_KEY` environment variable,
//! which must match the router's configured admin secret. The raw key is printed once and is
//! never recoverable afterwards.
//!
//! ```text
//! ADMIN_KEY=... create_api_key --env prod --label my-client --expires-at "7 days"
//! ADMIN_KEY=... create_api_key --env testnet --expires-at 1893456000   # explicit unix ts
//! ADMIN_KEY=... create_api_key --url http://localhost:8080             # local router
//! ```

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Arg, Command};

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Command::new("create_api_key")
        .about("Mint a Gas Killer API key against a deployed router's admin API")
        .arg(
            Arg::new("env")
                .long("env")
                .default_value("testnet")
                .help("Target environment: prod, testnet, or dev"),
        )
        .arg(
            Arg::new("url")
                .long("url")
                .help("Explicit router base URL, overriding --env (e.g. http://localhost:8080)"),
        )
        .arg(
            Arg::new("label")
                .long("label")
                .help("Human-readable label stored alongside the key"),
        )
        .arg(
            Arg::new("expires-at")
                .long("expires-at")
                .default_value("never")
                .help(
                    "When the key expires: \"never\", a relative duration like \"7 days\" or \
                     \"5 hours\", or an explicit unix timestamp in seconds",
                ),
        )
        .get_matches();

    let admin_key = std::env::var("ADMIN_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .context("ADMIN_KEY must be set to authenticate against the admin API")?;

    let base = match matches.get_one::<String>("url") {
        Some(url) => url.clone(),
        None => base_url_for_env(matches.get_one::<String>("env").expect("env has a default"))?
            .to_string(),
    };
    let base = base.trim_end_matches('/').to_string();
    let endpoint = format!("{base}/admin/keys");

    let label = matches
        .get_one::<String>("label")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let now = unix_now()?;
    let invalid_at = parse_expires_at(
        matches
            .get_one::<String>("expires-at")
            .expect("expires-at has a default"),
        now,
    )?;

    let body = serde_json::json!({ "label": label, "invalid_at": invalid_at });

    let resp = reqwest::Client::new()
        .post(&endpoint)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {admin_key}"),
        )
        .json(&body)
        .send()
        .await
        .with_context(|| format!("sending request to {endpoint}"))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("admin API at {endpoint} returned {status}: {text}");
    }

    let created: serde_json::Value =
        serde_json::from_str(&text).context("parsing admin API response")?;

    println!("API key created ({base}):");
    println!("  id:         {}", created["id"].as_str().unwrap_or("?"));
    println!("  key:        {}", created["key"].as_str().unwrap_or("?"));
    println!(
        "  label:      {}",
        created["label"].as_str().unwrap_or("(none)")
    );
    if let Some(created_at) = created["created_at"].as_i64() {
        println!("  created_at: {created_at}");
    }
    match created["invalid_at"].as_i64() {
        Some(ts) => println!("  expires_at: {ts} (unix)"),
        None => println!("  expires_at: never"),
    }
    println!();
    println!("Store this key now — it is not persisted in the clear and cannot be shown again.");

    Ok(())
}

/// Resolves an environment name to the router's public base URL.
fn base_url_for_env(env: &str) -> Result<&'static str> {
    match env.to_ascii_lowercase().as_str() {
        "prod" | "production" | "mainnet" => Ok("https://api.gaskiller.xyz/"),
        "testnet" | "dev" | "development" => Ok("https://testnet.gaskiller.xyz/"),
        other => bail!("unknown environment {other:?}; use prod, testnet, or dev (or pass --url)"),
    }
}

/// Current unix time in seconds.
fn unix_now() -> Result<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the unix epoch")?
        .as_secs() as i64)
}

/// Parses the `--expires-at` value into an optional unix timestamp (seconds). `None` means the
/// key never expires. `now` is injected so the parser is deterministically testable.
///
/// Accepted forms:
/// - `never` / `none` (case-insensitive) → no expiry
/// - a relative duration such as `7 days`, `5 hours`, `30m`, `2w` → `now` plus that duration
/// - a bare integer → an explicit unix timestamp (must be in the future)
fn parse_expires_at(raw: &str, now: i64) -> Result<Option<i64>> {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();

    if lower.is_empty() || lower == "never" || lower == "none" {
        return Ok(None);
    }

    // A bare integer is an explicit unix timestamp. A relative duration always carries a unit,
    // so this branch is unambiguous.
    if let Ok(ts) = trimmed.parse::<i64>() {
        if ts <= now {
            bail!("expiry timestamp {ts} is not in the future (now is {now})");
        }
        return Ok(Some(ts));
    }

    let secs = parse_relative_duration(&lower).with_context(|| {
        format!(
            "could not parse expiry {trimmed:?}; use \"never\", a duration like \"7 days\", \
             or a unix timestamp"
        )
    })?;
    let expires = now.checked_add(secs).context("expiry overflowed i64")?;
    Ok(Some(expires))
}

/// Parses a `<number><unit>` duration (whitespace between the two is optional) into seconds.
/// Units accept common singular, plural, and short forms.
fn parse_relative_duration(s: &str) -> Result<i64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num_str, unit_str) = s.split_at(split);
    if num_str.is_empty() {
        bail!("missing a number in duration {s:?}");
    }
    let num: i64 = num_str
        .parse()
        .context("duration count is not an integer")?;

    let mult = match unit_str.trim() {
        "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600,
        "d" | "day" | "days" => 86_400,
        "w" | "week" | "weeks" => 604_800,
        other => bail!("unknown time unit {other:?}; use seconds, minutes, hours, days, or weeks"),
    };

    num.checked_mul(mult).context("duration overflowed i64")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    #[test]
    fn env_names_resolve_to_public_urls() {
        assert_eq!(
            base_url_for_env("prod").unwrap(),
            "https://api.gaskiller.xyz/"
        );
        assert_eq!(
            base_url_for_env("PROD").unwrap(),
            "https://api.gaskiller.xyz/"
        );
        assert_eq!(
            base_url_for_env("testnet").unwrap(),
            "https://testnet.gaskiller.xyz/"
        );
        assert_eq!(
            base_url_for_env("dev").unwrap(),
            "https://testnet.gaskiller.xyz/"
        );
        assert!(base_url_for_env("staging").is_err());
    }

    #[test]
    fn never_and_none_mean_no_expiry() {
        assert_eq!(parse_expires_at("never", NOW).unwrap(), None);
        assert_eq!(parse_expires_at("NONE", NOW).unwrap(), None);
        assert_eq!(parse_expires_at("  never  ", NOW).unwrap(), None);
    }

    #[test]
    fn relative_durations_are_added_to_now() {
        assert_eq!(
            parse_expires_at("7 days", NOW).unwrap(),
            Some(NOW + 7 * 86_400)
        );
        assert_eq!(
            parse_expires_at("5 hours", NOW).unwrap(),
            Some(NOW + 5 * 3_600)
        );
        assert_eq!(parse_expires_at("30m", NOW).unwrap(), Some(NOW + 30 * 60));
        assert_eq!(
            parse_expires_at("2w", NOW).unwrap(),
            Some(NOW + 2 * 604_800)
        );
        assert_eq!(parse_expires_at("1 second", NOW).unwrap(), Some(NOW + 1));
    }

    #[test]
    fn explicit_future_timestamp_is_accepted() {
        assert_eq!(
            parse_expires_at(&(NOW + 500).to_string(), NOW).unwrap(),
            Some(NOW + 500)
        );
    }

    #[test]
    fn past_timestamp_is_rejected() {
        assert!(parse_expires_at(&(NOW - 1).to_string(), NOW).is_err());
        // A small bare integer reads as a 1970 timestamp, i.e. the past — rejected, not treated
        // as a relative duration.
        assert!(parse_expires_at("7", NOW).is_err());
    }

    #[test]
    fn unknown_unit_and_garbage_are_rejected() {
        assert!(parse_expires_at("5 fortnights", NOW).is_err());
        assert!(parse_expires_at("soon", NOW).is_err());
        assert!(parse_expires_at("days", NOW).is_err());
    }
}
