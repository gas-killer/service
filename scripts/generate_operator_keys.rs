//! Generates the per-operator key files the e2e stack expects, for stake sources that
//! have no key-provisioning container (STAKE_SOURCE=commitments). The EigenLayer setup
//! container writes these same files in eigenlayer mode; this binary is the drop-in
//! replacement so node volume mounts and the deploy binary see an identical layout:
//!
//!   testaccN.private.ecdsa.key.json  {"privateKey": "0x<32-byte hex>"}   (secp256k1)
//!   testaccN.private.bls.key.json    {"privateKey": "<decimal Fr>"}      (BN254)
//!
//! Existing files are left untouched, so re-runs are idempotent and a caller can
//! pre-seed deterministic keys.
//!
//! Env: `TEST_ACCOUNTS` (count, default 3), `OPERATOR_KEYS_DIR` (default
//! `../config/.nodes/operator_keys`).

use alloy::hex;
use alloy::signers::local::PrivateKeySigner;
use rand::RngCore;
use std::env;
use std::fs;
use std::path::PathBuf;

type DynError = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), DynError> {
    let count: usize = env::var("TEST_ACCOUNTS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(3);
    let dir = PathBuf::from(
        env::var("OPERATOR_KEYS_DIR")
            .unwrap_or_else(|_| "../config/.nodes/operator_keys".to_string()),
    );
    fs::create_dir_all(&dir)?;

    let mut rng = rand::rng();
    for i in 1..=count {
        let ecdsa_path = dir.join(format!("testacc{i}.private.ecdsa.key.json"));
        if ecdsa_path.exists() {
            println!("↩︎  {} exists, keeping it", ecdsa_path.display());
        } else {
            let signer = PrivateKeySigner::random();
            let contents = format!(
                "{{\"privateKey\": \"0x{}\"}}\n",
                hex::encode(signer.to_bytes())
            );
            fs::write(&ecdsa_path, contents)?;
            println!("🔑 Wrote {} ({})", ecdsa_path.display(), signer.address());
        }

        let bls_path = dir.join(format!("testacc{i}.private.bls.key.json"));
        if bls_path.exists() {
            println!("↩︎  {} exists, keeping it", bls_path.display());
        } else {
            use ark_ff::PrimeField;
            let mut bytes = [0u8; 32];
            rng.fill_bytes(&mut bytes);
            let fr = ark_bn254::Fr::from_le_bytes_mod_order(&bytes);
            fs::write(&bls_path, format!("{{\"privateKey\": \"{fr}\"}}\n"))?;
            println!("🔑 Wrote {}", bls_path.display());
        }
    }
    Ok(())
}
