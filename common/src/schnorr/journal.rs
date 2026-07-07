//! Durable, file-backed [`SpendJournal`] — the production enforcement of invariant N1
//! ("one partial per slot, ever"; see [`super::scheme`] and the plan doc §6.1).
//!
//! With deterministic nonce derivation a restarted node can recompute every secret
//! nonce, so the interactive mode's safe-by-amnesia property is gone: this journal is
//! what makes a restart unable to re-sign a consumed slot under a different context.
//!
//! # Format & durability
//!
//! An append-only file of fixed 44-byte records `slot₈ ‖ fingerprint₃₂ ‖ crc₄` (CRC over
//! the first 40 bytes, IEEE). [`FileSpendJournal::bind`] appends and **fsyncs before
//! returning `true`** — the write-ahead rule: a partial may hit the network the moment
//! `bind` returns. On open, records are replayed into memory; a torn/corrupt tail
//! (crash mid-append) is truncated away — safe, because a record whose `bind` never
//! returned `true` cannot have produced a partial.

use super::scheme::SpendJournal;
use std::collections::HashMap;
use std::fmt::{self, Debug};
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const RECORD_LEN: usize = 8 + 32 + 4;

/// CRC-32 (IEEE, reflected) — tiny local implementation to avoid a dependency for one
/// 40-byte checksum.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn encode_record(slot: u64, fingerprint: &[u8; 32]) -> [u8; RECORD_LEN] {
    let mut record = [0u8; RECORD_LEN];
    record[..8].copy_from_slice(&slot.to_be_bytes());
    record[8..40].copy_from_slice(fingerprint);
    let crc = crc32(&record[..40]);
    record[40..].copy_from_slice(&crc.to_be_bytes());
    record
}

struct Inner {
    file: File,
    bound: HashMap<u64, [u8; 32]>,
}

/// File-backed [`SpendJournal`]; see the module docs for format and crash semantics.
pub struct FileSpendJournal {
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl Debug for FileSpendJournal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileSpendJournal")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl FileSpendJournal {
    /// Opens (or creates) a journal file and replays it. A corrupt or torn tail is
    /// truncated; a corrupt record *before* valid ones is refused (that is data loss,
    /// not a crash artifact — refuse to sign rather than risk reuse).
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(&path)?;

        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(0))?;
        file.read_to_end(&mut bytes)?;

        let mut bound = HashMap::new();
        let mut valid_len = 0usize;
        for chunk in bytes.chunks(RECORD_LEN) {
            if chunk.len() < RECORD_LEN {
                break; // torn tail
            }
            let expected = u32::from_be_bytes(chunk[40..].try_into().expect("4 bytes"));
            if crc32(&chunk[..40]) != expected {
                break; // corrupt tail
            }
            let slot = u64::from_be_bytes(chunk[..8].try_into().expect("8 bytes"));
            let fingerprint: [u8; 32] = chunk[8..40].try_into().expect("32 bytes");
            bound.insert(slot, fingerprint);
            valid_len += RECORD_LEN;
        }
        // Anything after the valid prefix is a crash artifact of an append whose `bind`
        // never returned true — drop it so future appends stay record-aligned.
        if valid_len != bytes.len() {
            file.set_len(valid_len as u64)?;
            file.sync_all()?;
        }
        file.seek(SeekFrom::End(0))?;

        Ok(Self {
            path,
            inner: Mutex::new(Inner { file, bound }),
        })
    }

    /// Number of consumed slots (test/metrics aid).
    pub fn len(&self) -> usize {
        self.inner.lock().expect("journal lock").bound.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl SpendJournal for FileSpendJournal {
    fn bind(&self, slot: u64, fingerprint: &[u8; 32]) -> bool {
        let mut inner = self.inner.lock().expect("journal lock");
        if let Some(existing) = inner.bound.get(&slot) {
            return existing == fingerprint;
        }
        // Write-ahead: record + fsync BEFORE reporting the slot as signable. If any
        // step fails, refuse to sign (fail-closed) — never sign on an unpersisted bind.
        let record = encode_record(slot, fingerprint);
        if inner.file.write_all(&record).is_err() {
            return false;
        }
        if inner.file.sync_data().is_err() {
            return false;
        }
        inner.bound.insert(slot, *fingerprint);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "gas-killer-journal-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn bind_semantics() {
        let path = tmp("bind");
        let journal = FileSpendJournal::open(&path).unwrap();
        assert!(journal.is_empty());

        assert!(journal.bind(7, &[1u8; 32]), "fresh slot binds");
        assert!(journal.bind(7, &[1u8; 32]), "same context is idempotent");
        assert!(!journal.bind(7, &[2u8; 32]), "different context refused");
        assert!(journal.bind(8, &[2u8; 32]), "other slot unaffected");
        assert_eq!(journal.len(), 2);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn survives_restart() {
        let path = tmp("restart");
        {
            let journal = FileSpendJournal::open(&path).unwrap();
            assert!(journal.bind(1, &[0xAA; 32]));
            assert!(journal.bind(2, &[0xBB; 32]));
        }
        // "Restart": reopen and verify the bindings persist with the same semantics.
        let journal = FileSpendJournal::open(&path).unwrap();
        assert_eq!(journal.len(), 2);
        assert!(journal.bind(1, &[0xAA; 32]), "same context after restart");
        assert!(
            !journal.bind(1, &[0xCC; 32]),
            "REUSE ACROSS RESTART must be refused — this is invariant N1"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn torn_tail_is_truncated_and_journal_still_works() {
        let path = tmp("torn");
        {
            let journal = FileSpendJournal::open(&path).unwrap();
            assert!(journal.bind(1, &[0x11; 32]));
            assert!(journal.bind(2, &[0x22; 32]));
        }
        // Simulate a crash mid-append: half a record at the tail.
        {
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(&[0xDE; RECORD_LEN / 2]).unwrap();
        }
        let journal = FileSpendJournal::open(&path).unwrap();
        assert_eq!(journal.len(), 2, "valid prefix retained");
        assert!(!journal.bind(1, &[0x99; 32]));
        assert!(journal.bind(3, &[0x33; 32]), "appends realign after truncation");

        // And the realigned file replays cleanly again.
        drop(journal);
        let journal = FileSpendJournal::open(&path).unwrap();
        assert_eq!(journal.len(), 3);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn corrupt_tail_record_is_dropped() {
        let path = tmp("corrupt");
        {
            let journal = FileSpendJournal::open(&path).unwrap();
            assert!(journal.bind(1, &[0x11; 32]));
            assert!(journal.bind(2, &[0x22; 32]));
        }
        // Flip a byte in the LAST record's fingerprint (CRC now mismatches).
        {
            let mut bytes = std::fs::read(&path).unwrap();
            let offset = RECORD_LEN + 20;
            bytes[offset] ^= 0xFF;
            std::fs::write(&path, bytes).unwrap();
        }
        let journal = FileSpendJournal::open(&path).unwrap();
        assert_eq!(journal.len(), 1, "corrupt tail dropped, valid prefix kept");
        assert!(journal.bind(1, &[0x11; 32]));
        std::fs::remove_file(path).unwrap();
    }
}
