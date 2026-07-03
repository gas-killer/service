//! Prints a deterministic signature fixture for the Solidity cross-language
//! parity test (contracts/test/GasKillerSDK.t.sol).

use commonware_cryptography::{Hasher as _, Sha256, Signer as _};
use gas_killer_common::ecdsa::get_signer;

fn main() {
    let signer = get_signer("0x0000000000000000000000000000000000000000000000000000000000000001");
    let mut hasher = Sha256::new();
    hasher.update(b"gas-killer cross-language parity");
    let digest: [u8; 32] = hasher.finalize().as_ref().try_into().unwrap();
    let signature = signer.sign_digest(&digest);
    println!("address:   {}", signer.public_key());
    println!("digest:    0x{}", hex::encode(digest));
    println!("signature: 0x{}", hex::encode(signature.as_ref()));
}
