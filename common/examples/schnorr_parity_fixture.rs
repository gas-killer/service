//! Emits a deterministic aggregate-Schnorr fixture for the Solidity parity test
//! (`contracts/test/SchnorrStakeRegistry.t.sol`). Run:
//!
//! ```bash
//! cargo run -p gas-killer-common --example schnorr_parity_fixture
//! ```
//!
//! The printed values are pasted verbatim into the Solidity test so the *same* aggregate
//! signature this Rust protocol assembles is verified through the real on-chain path — the
//! byte-for-byte Rust⇄Solidity guarantee (mirrors the ECDSA `parity_fixture.rs`).

use alloy_primitives::{hex, keccak256};
use gas_killer_common::schnorr::musig::{Coordinator, Participant};
use gas_killer_common::schnorr::{Entropy, PrivateKey, PublicKey};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

fn seeded(seed: u64) -> impl Entropy {
    let mut rng = StdRng::seed_from_u64(seed);
    move |b: &mut [u8]| rng.fill_bytes(b)
}

fn h32(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

fn run(participants: &[Participant], idxs: &[usize], msg: &[u8; 32], seed: u64) {
    let mut fill = seeded(seed);
    let mut secnonces = Vec::new();
    let mut contributions = Vec::new();
    for &i in idxs {
        let (sec, pubn) = participants[i].new_nonce(&mut fill);
        contributions.push((participants[i].public_key(), pubn));
        secnonces.push((i, sec));
    }
    let ctx = Coordinator::build_context(&contributions, msg).expect("context");
    let partials: Vec<_> = secnonces
        .into_iter()
        .map(|(i, sec)| participants[i].sign(sec, &ctx).expect("signs"))
        .collect();
    let sig = Coordinator::assemble(&ctx, partials).expect("assembled");

    println!("    // signers: {idxs:?}");
    println!("    Xagg_x   = {}", h32(&ctx.x_agg.x_bytes()));
    println!("    Xagg_par = {}", ctx.x_agg.y_parity());
    println!("    sig_s    = {}", h32(&sig.s.to_bytes()));
    println!("    sig_R    = {}", sig.r_addr);
    assert!(
        gas_killer_common::schnorr::verify_aggregate(&ctx.x_agg, msg, &sig),
        "fixture must self-verify via the ecrecover identity"
    );
}

fn main() {
    let keys: Vec<PrivateKey> = (0..3).map(|i| PrivateKey::from_seed(1000 + i)).collect();
    let participants: Vec<Participant> = keys.iter().cloned().map(Participant::new).collect();
    let pubs: Vec<PublicKey> = participants.iter().map(|p| p.public_key()).collect();

    let message = keccak256(b"gas-killer schnorr parity fixture v1").0;

    println!("// ---- operators (register with these; PoP proves possession) ----");
    for (i, (p, sk)) in participants.iter().zip(&keys).enumerate() {
        let pk = p.public_key();
        let mut popfill = seeded(9000 + i as u64);
        let pop = sk.prove_possession(&mut popfill);
        assert!(pk.verify_possession(&pop));
        println!("    // operator {i}  addr {}", pk.eth_address());
        println!("    op_x[{i}]    = {}", h32(&pk.x_bytes()));
        println!("    op_y[{i}]    = {}", h32(&pk.y_bytes()));
        println!("    pop_s[{i}]   = {}", h32(&pop.0.s.to_bytes()));
        println!("    pop_R[{i}]   = {}", pop.0.r_addr);
    }

    // Aggregate of all operators (checkpointed X_all on-chain).
    let all = PublicKey::aggregate(&pubs).unwrap();
    println!("\n// ---- X_all = Σ X_i (registry stores this checkpointed) ----");
    println!("    Xall_x   = {}", h32(&all.x_bytes()));
    println!("    Xall_y   = {}", h32(&all.y_bytes()));

    println!("\n// ---- message ----");
    println!("    message  = {}", h32(&message));

    println!("\n// ---- full participation (no non-signers) ----");
    run(&participants, &[0, 1, 2], &message, 1);

    println!("\n// ---- subset: operator 1 offline (non-signer) ----");
    run(&participants, &[0, 2], &message, 2);
}
