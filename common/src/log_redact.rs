//! Scrubs credentials out of URLs in log output.
//!
//! RPC providers put the API key in the URL path (Alchemy `/v2/<key>`, Infura `/v3/<key>`) or
//! query (drpc `?dkey=<key>`), and dependencies log that URL as-is: alloy's `ReqwestTransport`
//! span carries it on every request, and reqwest errors print it. Redacting at the writer catches
//! every such line regardless of which crate emitted it, at the cost of hiding non-secret paths
//! too. Only a URL rendered with its `scheme://` is caught; a `Debug`-formatted `Url` struct is not.

use std::borrow::Cow;
use std::io::{self, Write};

const REDACTED: &str = "<redacted>";

/// Ends a URL in a log line: whitespace, quoting and closing delimiters, or an ANSI escape.
fn ends_url(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ')' | ']' | '}' | '>' | '\x1b')
}

/// Ends a URL's authority. `]` is excluded so an IPv6 host stays whole.
fn ends_authority(c: char) -> bool {
    matches!(c, '/' | '?' | '#') || (ends_url(c) && c != ']')
}

/// Replaces the userinfo, path and query of every URL in `line`, keeping scheme, host and port so
/// a line still says which endpoint it concerns.
pub fn redact_urls(line: &str) -> Cow<'_, str> {
    if !line.contains("://") {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(scheme_end) = rest.find("://") {
        let (head, tail) = rest.split_at(scheme_end + 3);
        out.push_str(head);

        let authority_len = tail.find(ends_authority).unwrap_or(tail.len());
        let (authority, tail) = tail.split_at(authority_len);
        match authority.rfind('@') {
            Some(at) => {
                out.push_str(REDACTED);
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }

        let path_len = tail.find(ends_url).unwrap_or(tail.len());
        let (path, tail) = tail.split_at(path_len);
        if path.is_empty() || path == "/" {
            out.push_str(path);
        } else {
            out.push('/');
            out.push_str(REDACTED);
        }
        rest = tail;
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Wraps a log writer so every write has its URLs redacted. `tracing_subscriber::fmt` writes each
/// event in a single call, so a URL is never split across writes.
pub struct RedactingWriter<W>(pub W);

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        self.0.write_all(redact_urls(&text).as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// A `tracing_subscriber::fmt` writer: stdout with URLs redacted.
pub fn redacting_stdout() -> RedactingWriter<io::Stdout> {
    RedactingWriter(io::stdout())
}

/// Installs the process-wide subscriber: DEBUG, plain text, URLs redacted. It must be the global
/// default, since the events that carry RPC URLs fire on runtime worker threads.
pub fn init_logging() -> Result<(), tracing::subscriber::SetGlobalDefaultError> {
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(redacting_stdout)
            .finish(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_key_is_redacted_and_the_host_kept() {
        let line = "ReqwestTransport{url=https://eth-sepolia.g.alchemy.com/v2/abc123}:request{method_names=eth_blockNumber}: pooling";
        assert_eq!(
            redact_urls(line),
            "ReqwestTransport{url=https://eth-sepolia.g.alchemy.com/<redacted>}:request{method_names=eth_blockNumber}: pooling"
        );
    }

    #[test]
    fn a_query_key_is_redacted() {
        assert_eq!(
            redact_urls("url=https://lb.drpc.org/ogrpc?network=sepolia&dkey=secret done"),
            "url=https://lb.drpc.org/<redacted> done"
        );
    }

    #[test]
    fn userinfo_is_redacted() {
        assert_eq!(
            redact_urls("error sending request for url (https://user:pass@rpc.example.com:8545/)"),
            "error sending request for url (https://<redacted>@rpc.example.com:8545/)"
        );
    }

    #[test]
    fn a_url_without_a_path_is_unchanged() {
        let line = "sim=http://sim-node:8545 ws=ws://[::1]:8546/ ok";
        assert_eq!(redact_urls(line), line);
    }

    #[test]
    fn every_url_in_a_line_is_redacted() {
        assert_eq!(
            redact_urls("a=https://x.io/v3/k1 b=\"https://y.io/v2/k2\""),
            "a=https://x.io/<redacted> b=\"https://y.io/<redacted>\""
        );
    }

    #[test]
    fn an_ansi_escape_ends_the_url() {
        assert_eq!(
            redact_urls("\x1b[3murl\x1b[0m=https://x.io/v2/k\x1b[2m}"),
            "\x1b[3murl\x1b[0m=https://x.io/<redacted>\x1b[2m}"
        );
    }

    #[test]
    fn the_writer_redacts_what_it_forwards() {
        let mut writer = RedactingWriter(Vec::new());
        let line = b"url=https://x.io/v2/key\n";
        assert_eq!(writer.write(line).unwrap(), line.len());
        assert_eq!(writer.0, b"url=https://x.io/<redacted>\n");
    }
}
