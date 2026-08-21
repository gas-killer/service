//! Prints the operator set as read from the Commitments `SchnorrCommitmentsAdapter` —
//! the exact `QuorumInfo` view the node/router bootstrap consumes when
//! `STAKE_SOURCE=commitments`. Handy for verifying a deployment's published set,
//! sockets, and BN254 key decoding without booting a node.
//!
//! Usage:
//!   HTTP_RPC=http://localhost:8545 \
//!   AVS_DEPLOYMENT_PATH=config/.nodes/avs_deploy.json \
//!   cargo run -p gas-killer-common --example commitments_operator_set

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let states = gas_killer_common::commitments::get_operator_states_commitments().await?;
    for quorum in &states {
        println!(
            "quorum {} — {} operator(s), threshold {}, total stake {}",
            quorum.quorum_number, quorum.operator_count, quorum.threshold, quorum.total_stake
        );
        for op in &quorum.operators {
            println!(
                "  {} stake={} socket={} bn254={}",
                op.address,
                op.stake,
                op.socket.as_deref().unwrap_or("<none>"),
                if op.pub_keys.is_some() { "ok" } else { "INVALID" },
            );
        }
    }
    Ok(())
}
