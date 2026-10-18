//! UNBOUNDED_V3 program distribution: the hash-addressed path.
//!
//! The boot-time slots (`GK_GUEST_PROGRAM[_N]`, see
//! [`crate::local_exec_shim::gkvm_host_from_env`]) make every program an
//! operator serves a line of operator config: a new consumer means a Helm
//! edit, a restart and a re-download on the router and every node. This
//! module is the other half of the design — the Qwen overlay channel,
//! generalized: the CONSUMER names its program, and the OPERATOR fetches it
//! by hash the first time a task needs it.
//!
//! * [`GuestProgramResolver`] reads `programHash()` — the `IGkVmConsumer`
//!   interface the SDK scaffold implements — or the `PROGRAM_HASH()` public
//!   constant the example consumers expose, from the consumer AT THE TASK'S
//!   BLOCK, so every operator resolves the same program for the same task. A
//!   zero hash, a revert or empty returndata means "this consumer exposes
//!   nothing", and the `GK_GUEST_VM_CONSUMERS` registry stays the binding.
//! * [`GuestProgramFetcher`] turns a hash into bytes: the on-disk cache
//!   (`GK_GUEST_CACHE_DIR`), then each mirror in `GK_GUEST_MIRRORS` as
//!   `<base>/<hash hex>.elf`. The bytes are keccak-verified against the hash
//!   and parsed by SP1's loader ([`LoadedGuestProgram::from_bytes`]) before
//!   they are installed on the live [`GkvmHost`] — a mirror can only ever
//!   affect availability, never which program runs, and a mirror serving the
//!   wrong bytes is skipped, never trusted.
//!
//! Determinism is unchanged from the boot-time path: operators that have (or
//! can fetch) the program produce the same payload; an operator that cannot
//! fetch it abstains, exactly like one without the slot configured.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use alloy::eips::BlockId;
use alloy::primitives::{Address, B256};
use alloy::providers::Provider;
use anyhow::{Context, Result, anyhow, bail};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::local_exec_shim::{GkvmHost, LoadedGuestProgram};

/// Default cap on a fetched program (`GK_GUEST_MAX_BYTES`): 32 MiB — well
/// above the ~1 MB MicroPython guests and the C guests, well below anything
/// an operator should download on a hostile consumer's say-so.
pub const DEFAULT_MAX_BYTES: u64 = 32 * 1024 * 1024;
/// Default timeout of one mirror request (`GK_GUEST_FETCH_TIMEOUT_SECS`).
pub const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Resolver entries kept before the map is cleared. Bounded memory; the hit
/// that matters is prewarm → validation of one task at one block.
const RESOLVER_CACHE_ENTRIES: usize = 4096;

alloy::sol! {
    /// The consumer-side getters an operator reads: solidity-sdk
    /// `src/gkvm/IGkVmConsumer.sol`, plus the `PROGRAM_HASH` public constant
    /// the example consumers expose.
    #[sol(rpc)]
    interface IGkVmConsumer {
        function programHash() external view returns (bytes32);
        function artifactRoot() external view returns (bytes32);
        function PROGRAM_HASH() external view returns (bytes32);
    }
}

/// The chain could not be asked which program a consumer runs: a transport
/// failure, not a consumer that exposes nothing. The router's ingress maps it
/// to "service unavailable" rather than "program unavailable".
#[derive(Debug)]
pub struct ChainReadFailed(pub String);

impl std::fmt::Display for ChainReadFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ChainReadFailed {}

/// Resolves which guest program a consumer runs by reading the consumer.
#[derive(Default)]
pub struct GuestProgramResolver {
    cache: StdMutex<HashMap<(Address, u64), Option<B256>>>,
}

impl GuestProgramResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// The program `consumer` exposes at `block_height`: `programHash()`, else
    /// `PROGRAM_HASH()`; `None` when it exposes neither, or exposes zero.
    /// `Err` ([`ChainReadFailed`]) only when the chain could not be asked —
    /// the caller must not guess then.
    pub async fn program_hash<P: Provider + Clone>(
        &self,
        provider: &P,
        consumer: Address,
        block_height: u64,
    ) -> Result<Option<B256>> {
        let key = (consumer, block_height);
        if let Some(hit) = self
            .cache
            .lock()
            .expect("guest program resolver cache poisoned")
            .get(&key)
        {
            return Ok(*hit);
        }

        let contract = IGkVmConsumer::new(consumer, provider.clone());
        let block = BlockId::number(block_height);
        let resolved = match contract.programHash().block(block).call().await {
            Ok(hash) => Some(hash),
            Err(e) if exposes_nothing(&e) => {
                match contract.PROGRAM_HASH().block(block).call().await {
                    Ok(hash) => Some(hash),
                    Err(e) if exposes_nothing(&e) => None,
                    Err(e) => {
                        return Err(anyhow::Error::new(ChainReadFailed(format!(
                            "reading PROGRAM_HASH() of {consumer} at block {block_height}: {e}"
                        ))));
                    }
                }
            }
            Err(e) => {
                return Err(anyhow::Error::new(ChainReadFailed(format!(
                    "reading programHash() of {consumer} at block {block_height}: {e}"
                ))));
            }
        };
        let resolved = resolved.filter(|hash| !hash.is_zero());
        debug!(
            consumer = %consumer,
            block_height,
            program = ?resolved,
            "guest program resolved from the consumer"
        );

        let mut cache = self
            .cache
            .lock()
            .expect("guest program resolver cache poisoned");
        if cache.len() >= RESOLVER_CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert(key, resolved);
        Ok(resolved)
    }
}

/// A call outcome that means "no such getter here": a revert (no function, no
/// fallback), empty returndata (a fallback that returns nothing), or
/// returndata that is not one word. Everything else is the chain not
/// answering.
fn exposes_nothing(e: &alloy::contract::Error) -> bool {
    match e {
        alloy::contract::Error::ZeroData(..) | alloy::contract::Error::AbiError(_) => true,
        alloy::contract::Error::TransportError(transport) => transport.is_error_resp(),
        _ => false,
    }
}

/// How [`GuestProgramFetcher::ensure_installed`] satisfied a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// The host already served the program.
    Already,
    /// Installed from the on-disk cache: a previous fetch, or a file an
    /// operator dropped there by hand.
    FromCache(PathBuf),
    /// Fetched from this URL, verified and installed.
    FromMirror(String),
}

/// Fetches guest programs by hash and installs them on a running host.
pub struct GuestProgramFetcher {
    mirrors: Vec<String>,
    cache_dir: Option<PathBuf>,
    max_bytes: u64,
    client: reqwest::Client,
    /// One install at a time per hash: concurrent tasks naming the same new
    /// program share one download instead of racing several.
    installs: Mutex<HashMap<B256, Arc<Mutex<()>>>>,
}

impl GuestProgramFetcher {
    /// `mirrors` are http(s) base URLs, tried in order; `cache_dir` keeps
    /// fetched programs across restarts; `max_bytes` refuses larger programs;
    /// `timeout` bounds one mirror request.
    pub fn new(
        mirrors: Vec<String>,
        cache_dir: Option<PathBuf>,
        max_bytes: u64,
        timeout: Duration,
    ) -> Result<Self> {
        let mut normalized: Vec<String> = Vec::with_capacity(mirrors.len());
        for mirror in &mirrors {
            let base = mirror.trim().trim_end_matches('/').to_string();
            if !(base.starts_with("http://") || base.starts_with("https://")) {
                bail!("guest program mirror {mirror:?}: expected an http(s) base URL");
            }
            if !normalized.contains(&base) {
                normalized.push(base);
            }
        }
        if max_bytes == 0 {
            bail!("the guest program size cap must be positive");
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("building the guest program HTTP client")?;
        Ok(Self {
            mirrors: normalized,
            cache_dir,
            max_bytes,
            client,
            installs: Mutex::new(HashMap::new()),
        })
    }

    /// `GK_GUEST_MIRRORS` (comma and/or whitespace separated base URLs),
    /// `GK_GUEST_CACHE_DIR`, `GK_GUEST_MAX_BYTES`, `GK_GUEST_FETCH_TIMEOUT_SECS`.
    /// `Ok(None)` when neither mirrors nor a cache dir is configured; empty
    /// values count as unset, as for every other `GK_GUEST_*` variable.
    pub fn from_env() -> Result<Option<Self>> {
        let var = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let mirrors: Vec<String> = var("GK_GUEST_MIRRORS")
            .map(|raw| {
                raw.split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|entry| !entry.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let cache_dir = var("GK_GUEST_CACHE_DIR").map(PathBuf::from);
        if mirrors.is_empty() && cache_dir.is_none() {
            return Ok(None);
        }
        let max_bytes = match var("GK_GUEST_MAX_BYTES") {
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|e| anyhow!("invalid GK_GUEST_MAX_BYTES {raw:?}: {e}"))?,
            None => DEFAULT_MAX_BYTES,
        };
        let timeout = match var("GK_GUEST_FETCH_TIMEOUT_SECS") {
            Some(raw) => Duration::from_secs(
                raw.parse::<u64>()
                    .map_err(|e| anyhow!("invalid GK_GUEST_FETCH_TIMEOUT_SECS {raw:?}: {e}"))?,
            ),
            None => DEFAULT_FETCH_TIMEOUT,
        };
        Self::new(mirrors, cache_dir, max_bytes, timeout).map(Some)
    }

    /// The mirror base URLs, in the order they are tried.
    pub fn mirrors(&self) -> &[String] {
        &self.mirrors
    }

    /// Where fetched programs are kept, if anywhere.
    pub fn cache_dir(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }

    /// The object a program is published as: its hash in lowercase hex, `.elf`.
    /// `gk publish` writes exactly this name.
    pub fn object_name(hash: &B256) -> String {
        format!("{hash:x}.elf")
    }

    /// `<base>/<hash hex>.elf`.
    pub fn program_url(base: &str, hash: &B256) -> String {
        format!("{}/{}", base.trim_end_matches('/'), Self::object_name(hash))
    }

    fn cached_path(&self, hash: &B256) -> Option<PathBuf> {
        self.cache_dir
            .as_ref()
            .map(|dir| dir.join(Self::object_name(hash)))
    }

    /// Makes `host` serve the program committed to by `hash`: already
    /// installed, from the cache, or from the first mirror whose bytes verify.
    /// `Err` when none did — the caller abstains.
    pub async fn ensure_installed(&self, host: &GkvmHost, hash: B256) -> Result<Installed> {
        if host.has_program(&hash) {
            return Ok(Installed::Already);
        }
        let lock = {
            let mut installs = self.installs.lock().await;
            Arc::clone(installs.entry(hash).or_default())
        };
        let _installing = lock.lock().await;
        if host.has_program(&hash) {
            return Ok(Installed::Already);
        }

        if let Some(path) = self.cached_path(&hash) {
            match std::fs::read(&path) {
                Ok(bytes) => match load(bytes, hash, path.display().to_string()).await {
                    Ok(program) => {
                        host.install_program(program);
                        info!(program = %hash, path = %path.display(), "guest program installed from the cache");
                        return Ok(Installed::FromCache(path));
                    }
                    Err(e) => {
                        warn!(program = %hash, path = %path.display(), error = %e, "cached guest program does not verify: removing it");
                        let _ = std::fs::remove_file(&path);
                    }
                },
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => {
                    warn!(program = %hash, path = %path.display(), error = %e, "cached guest program could not be read");
                }
            }
        }

        if self.mirrors.is_empty() {
            bail!(
                "guest program {hash} is not installed and no mirror is configured (GK_GUEST_MIRRORS)"
            );
        }
        let mut failures = Vec::with_capacity(self.mirrors.len());
        for base in &self.mirrors {
            let url = Self::program_url(base, &hash);
            let bytes = match self.download(&url).await {
                Ok(bytes) => bytes,
                Err(e) => {
                    warn!(program = %hash, url = %url, error = %format!("{e:#}"), "guest program mirror did not serve the program");
                    failures.push(format!("{url}: {e:#}"));
                    continue;
                }
            };
            match load(bytes, hash, url.clone()).await {
                Ok(program) => {
                    if let Some(path) = self.cached_path(&hash)
                        && let Err(e) = write_atomically(&path, &program.elf)
                    {
                        warn!(program = %hash, path = %path.display(), error = %e, "guest program fetched but not cached");
                    }
                    host.install_program(program);
                    info!(program = %hash, url = %url, "guest program fetched, verified and installed");
                    return Ok(Installed::FromMirror(url));
                }
                Err(e) => {
                    // The wrong bytes under the right name: this mirror is not
                    // trusted for this program, the next one may be.
                    warn!(program = %hash, url = %url, error = %e, "guest program mirror served bytes that do not verify");
                    failures.push(format!("{url}: {e}"));
                }
            }
        }
        bail!(
            "guest program {hash} is not installed and no mirror served it: {}",
            failures.join("; ")
        )
    }

    /// GET `url` into memory, refusing anything over the size cap.
    async fn download(&self, url: &str) -> Result<Vec<u8>> {
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .context("request failed")?;
        let status = response.status();
        if !status.is_success() {
            bail!("HTTP {status}");
        }
        if let Some(len) = response.content_length()
            && len > self.max_bytes
        {
            bail!("{len} bytes exceeds the {} byte cap", self.max_bytes);
        }
        let mut bytes =
            Vec::with_capacity(response.content_length().unwrap_or(0).min(self.max_bytes) as usize);
        while let Some(chunk) = response.chunk().await.context("read failed")? {
            if (bytes.len() + chunk.len()) as u64 > self.max_bytes {
                bail!("exceeds the {} byte cap", self.max_bytes);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

/// keccak + SP1 loader off the async threads (a few ms for a 1 MB ELF, but
/// nothing the 2-worker runtime should host).
async fn load(bytes: Vec<u8>, hash: B256, origin: String) -> Result<LoadedGuestProgram> {
    tokio::task::spawn_blocking(move || LoadedGuestProgram::from_bytes(bytes, hash, &origin))
        .await
        .context("guest program loader task panicked")?
        .map_err(|e| anyhow!("{e}"))
}

/// Write-then-rename, so a reader never sees a partial program.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "no parent directory"))?;
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("program");
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::keccak256;
    use gas_analyzer::GuestProgramSet;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// gas-analyzer's committed M1 guest, the one the gkvm e2e tests install.
    const HELLO_C: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gkvm/hello-c.elf"
    ));

    fn fetcher(mirrors: Vec<String>, cache_dir: Option<PathBuf>) -> GuestProgramFetcher {
        GuestProgramFetcher::new(
            mirrors,
            cache_dir,
            DEFAULT_MAX_BYTES,
            Duration::from_secs(5),
        )
        .expect("valid configuration")
    }

    fn empty_host() -> GkvmHost {
        GkvmHost::new(GuestProgramSet::default())
    }

    /// A one-route HTTP server answering every request with `status` and
    /// `body`; returns its base URL.
    async fn serve(status: &'static str, body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let body = body.clone();
                tokio::spawn(async move {
                    let mut request = [0u8; 4096];
                    let _ = socket.read(&mut request).await;
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn objects_are_named_by_hash() {
        let hash = keccak256(HELLO_C);
        let name = GuestProgramFetcher::object_name(&hash);
        assert_eq!(name.len(), 64 + 4);
        assert!(name.ends_with(".elf") && !name.starts_with("0x"));
        assert_eq!(
            GuestProgramFetcher::program_url("https://mirror.example/programs/", &hash),
            format!("https://mirror.example/programs/{name}")
        );
    }

    #[test]
    fn mirrors_are_normalized_and_validated() {
        let f = fetcher(
            vec![
                "https://a.example/p/".into(),
                " https://a.example/p ".into(),
                "http://b.example".into(),
            ],
            None,
        );
        assert_eq!(f.mirrors(), ["https://a.example/p", "http://b.example"]);
        assert!(
            GuestProgramFetcher::new(
                vec!["s3://bucket/p".into()],
                None,
                DEFAULT_MAX_BYTES,
                Duration::from_secs(1)
            )
            .is_err()
        );
        assert!(GuestProgramFetcher::new(vec![], None, 0, Duration::from_secs(1)).is_err());
    }

    #[tokio::test]
    async fn without_mirrors_or_cache_nothing_installs() {
        let f = fetcher(vec![], None);
        let host = empty_host();
        let err = f
            .ensure_installed(&host, keccak256(HELLO_C))
            .await
            .expect_err("nothing to fetch from");
        assert!(err.to_string().contains("no mirror is configured"), "{err}");
    }

    #[tokio::test]
    async fn a_cached_program_installs_without_a_mirror_and_a_bad_one_is_removed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hash = keccak256(HELLO_C);
        let good = dir.path().join(GuestProgramFetcher::object_name(&hash));
        std::fs::write(&good, HELLO_C).unwrap();
        let bogus_hash = B256::repeat_byte(0xab);
        let bogus = dir
            .path()
            .join(GuestProgramFetcher::object_name(&bogus_hash));
        std::fs::write(&bogus, b"not the program").unwrap();

        let f = fetcher(vec![], Some(dir.path().to_path_buf()));
        let host = empty_host();
        assert_eq!(
            f.ensure_installed(&host, hash).await.expect("from cache"),
            Installed::FromCache(good)
        );
        assert!(host.has_program(&hash));
        assert_eq!(
            f.ensure_installed(&host, hash).await.expect("installed"),
            Installed::Already
        );

        let err = f
            .ensure_installed(&host, bogus_hash)
            .await
            .expect_err("wrong bytes never install");
        assert!(err.to_string().contains("no mirror is configured"), "{err}");
        assert!(!bogus.exists(), "a file that does not verify is removed");
        assert!(!host.has_program(&bogus_hash));
    }

    #[tokio::test]
    async fn the_first_mirror_whose_bytes_verify_wins_and_is_cached() {
        let hash = keccak256(HELLO_C);
        let missing = serve("404 Not Found", b"no such object".to_vec()).await;
        let wrong = serve("200 OK", b"the wrong program under the right name".to_vec()).await;
        let right = serve("200 OK", HELLO_C.to_vec()).await;
        let dir = tempfile::tempdir().expect("tempdir");

        let f = fetcher(
            vec![missing.clone(), wrong.clone(), right.clone()],
            Some(dir.path().to_path_buf()),
        );
        let host = empty_host();
        let installed = f.ensure_installed(&host, hash).await.expect("third mirror");
        assert_eq!(
            installed,
            Installed::FromMirror(GuestProgramFetcher::program_url(&right, &hash))
        );
        assert!(host.has_program(&hash));
        let cached = dir.path().join(GuestProgramFetcher::object_name(&hash));
        assert_eq!(std::fs::read(cached).expect("cached copy"), HELLO_C);

        // A second host with the same cache dir needs no mirror at all.
        let offline = fetcher(vec![], Some(dir.path().to_path_buf()));
        let other = empty_host();
        assert!(matches!(
            offline.ensure_installed(&other, hash).await.expect("cache"),
            Installed::FromCache(_)
        ));
    }

    #[tokio::test]
    async fn no_mirror_serving_the_program_means_abstain() {
        let hash = keccak256(HELLO_C);
        let wrong = serve("200 OK", b"garbage".to_vec()).await;
        let f = fetcher(vec![wrong], None);
        let host = empty_host();
        let err = f
            .ensure_installed(&host, hash)
            .await
            .expect_err("no mirror served it");
        let text = format!("{err:#}");
        assert!(text.contains("no mirror served it"), "{text}");
        assert!(text.contains("hashes to"), "names the mismatch: {text}");
        assert!(!host.has_program(&hash));
    }

    #[tokio::test]
    async fn the_size_cap_is_enforced_before_bytes_are_trusted() {
        let hash = keccak256(HELLO_C);
        let right = serve("200 OK", HELLO_C.to_vec()).await;
        let capped = GuestProgramFetcher::new(vec![right], None, 1024, Duration::from_secs(5))
            .expect("valid");
        let host = empty_host();
        let err = capped
            .ensure_installed(&host, hash)
            .await
            .expect_err("over the cap");
        assert!(format!("{err:#}").contains("byte cap"), "{err:#}");
        assert!(!host.has_program(&hash));
    }
}
