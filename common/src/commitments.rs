//! Commitments-protocol operator-set source.
//!
//! The Commitments stake root replaces EigenLayer's registration lifecycle: operators
//! stake through the Commitments `CommitmentManager`/`OperatorRegistry` and publish
//! their Schnorr key, BN254 p2p identity, and socket through the
//! `SchnorrCommitmentsAdapter` (the `SchnorrStakeRegistry`'s owner). This module is the
//! off-chain read side: one `eth_call` to `adapter.getOperatorSet()` replaces the
//! EigenLayer event scan `EigenStakingClient` performs, reassembled into the exact
//! [`QuorumInfo`] shape the router/node bootstrap already consumes.
//!
//! Selected by `STAKE_SOURCE=commitments` (see [`crate::config::stake_source`]); the
//! EigenLayer path remains the default until the migration flips it.

use crate::bindings::schnorrcommitmentsadapter::SchnorrCommitmentsAdapter;
use alloy::primitives::{Address, U256};
use alloy::providers::ProviderBuilder;
use commonware_avs_eigenlayer::{CommonwarePublicKeys, OperatorInfo, QuorumInfo};
use serde::Deserialize;
use std::env;
use tracing::{info, warn};

/// The single quorum this deployment operates on (mirrors `QUORUM_NUMBERS`).
const QUORUM_NUMBER: u8 = 0;

#[derive(Deserialize)]
struct Deployment {
    addresses: DeploymentAddresses,
}

#[derive(Deserialize)]
struct DeploymentAddresses {
    #[serde(rename = "schnorrCommitmentsAdapter")]
    schnorr_commitments_adapter: String,
}

/// Fetches the operator set from the Commitments-backed adapter.
///
/// Reads the same environment surface as the EigenLayer path:
/// - `HTTP_RPC`: HTTP RPC endpoint
/// - `AVS_DEPLOYMENT_PATH`: deployment JSON carrying `addresses.schnorrCommitmentsAdapter`
/// - `QUORUM_THRESHOLD`/`THRESHOLD_DENOMINATOR`: the count-threshold fraction (kept in
///   lockstep with the on-chain registry threshold, exactly as before)
///
/// Operators whose live registry weight is zero (queued behind a notice window, or
/// drained by a sync) are excluded from the participant set, matching the on-chain
/// signer-set semantics.
///
/// # Errors
/// Returns an error if environment variables are missing, the deployment JSON is
/// malformed, the RPC call fails, or a published BN254 key does not decode.
pub async fn get_operator_states_commitments()
-> Result<Vec<QuorumInfo>, Box<dyn std::error::Error>> {
    let http_rpc = env::var("HTTP_RPC").expect("HTTP_RPC must be set");
    let deployment_path =
        env::var("AVS_DEPLOYMENT_PATH").expect("AVS_DEPLOYMENT_PATH must be set");

    let contents = std::fs::read_to_string(&deployment_path)?;
    let deployment: Deployment = serde_json::from_str(&contents)?;
    let adapter_address: Address = deployment.addresses.schnorr_commitments_adapter.parse()?;

    let provider = ProviderBuilder::new().connect_http(url::Url::parse(&http_rpc)?);
    let adapter = SchnorrCommitmentsAdapter::new(adapter_address, provider);

    let set = adapter.getOperatorSet().call().await?;

    let mut operators = Vec::new();
    let mut total_stake = U256::ZERO;
    for view in &set {
        if view.weight == U256::ZERO {
            info!(
                operator = %view.operator,
                "skipping operator with zero registry weight (pending or drained)"
            );
            continue;
        }

        // Coordinate order is fixed by the adapter's publishing convention:
        // blsG1 = [x, y], blsG2 = [x_c0, x_c1, y_c0, y_c1] — the same (c0, c1)
        // order `CommonwarePublicKeys::from_string_coordinates` expects.
        let g2: Vec<String> = view.info.blsG2.iter().map(U256::to_string).collect();
        let g1: Vec<String> = view.info.blsG1.iter().map(U256::to_string).collect();
        let pub_keys = CommonwarePublicKeys::from_string_coordinates(
            &g2[0], &g2[1], &g2[2], &g2[3], &g1[0], &g1[1],
        );
        if pub_keys.is_none() {
            warn!(operator = %view.operator, "published BN254 key is not a valid curve point");
        }

        total_stake += view.weight;
        operators.push(OperatorInfo {
            address: view.operator,
            stake: view.weight,
            pub_keys,
            socket: Some(view.info.socket.clone()),
            quorum_number: QUORUM_NUMBER,
        });
    }

    let (threshold_num, threshold_den) = crate::config::quorum_threshold_fraction();
    let operator_count = operators.len();
    let threshold = (operator_count as u64 * threshold_num).div_ceil(threshold_den) as usize;

    info!(
        operator_count,
        threshold,
        %total_stake,
        adapter = %adapter_address,
        "loaded operator set from Commitments adapter"
    );

    Ok(vec![QuorumInfo {
        quorum_number: QUORUM_NUMBER,
        operator_count,
        threshold,
        total_stake,
        operators,
    }])
}
