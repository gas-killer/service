use crate::handlers::creator::create_creator;
use crate::handlers::executor::create_executor;
use crate::handlers::listening_creator::create_listening_creator_with_server;
use crate::handlers::{TaskCreator, TaskCreatorEnum};
use crate::validator::Validator;
use crate::wire::{self, aggregation::Payload};

use bn254::{Bn254, G1PublicKey, PublicKey, Signature as Bn254Signature};
use bytes::Bytes;
use commonware_codec::{EncodeSize, ReadExt, Write};
use commonware_cryptography::{Hasher, Sha256, Verifier};
use commonware_macros::select;
use commonware_p2p::{Receiver, Sender};
use commonware_runtime::Clock;
use commonware_utils::hex;
use dotenv::dotenv;
use std::{collections::HashMap, time::Duration};
use tracing::info;

pub struct Orchestrator<E: Clock> {
    runtime: E,
    #[allow(dead_code)]
    signer: Bn254,
    aggregation_frequency: Duration,
    contributors: Vec<PublicKey>,
    g1_map: HashMap<PublicKey, G1PublicKey>, // g2 (PublicKey) -> g1 (PublicKey)
    ordered_contributors: HashMap<PublicKey, usize>,
    t: usize,
}

impl<E: Clock> Orchestrator<E> {
    pub async fn new(
        runtime: E,
        signer: Bn254,
        aggregation_frequency: Duration,
        mut contributors: Vec<PublicKey>,
        g1_map: HashMap<PublicKey, G1PublicKey>,
        t: usize,
    ) -> Self {
        dotenv().ok();

        contributors.sort();
        let mut ordered_contributors = HashMap::new();
        for (idx, contributor) in contributors.iter().enumerate() {
            ordered_contributors.insert(contributor.clone(), idx);
        }

        Self {
            runtime,
            signer,
            aggregation_frequency,
            contributors,
            g1_map,
            ordered_contributors,
            t,
        }
    }

    pub async fn run(
        self,
        mut sender: impl Sender,
        mut receiver: impl Receiver<PublicKey = PublicKey>,
    ) {
        let mut hasher = Sha256::new();
        let mut signatures = HashMap::new();
        let task_creator: TaskCreatorEnum;
        // Check if INGRESS flag is set to determine which creator to use
        let use_ingress = std::env::var("INGRESS").unwrap_or_default().to_lowercase() == "true";
        if use_ingress {
            info!("Using ListeningCreator with HTTP server on port 8080");
            let listening_creator =
                create_listening_creator_with_server("0.0.0.0:8080".to_string())
                    .await
                    .unwrap();
            task_creator = TaskCreatorEnum::ListeningCreator(listening_creator);
        } else {
            info!("Using Creator without ingress");
            let creator = create_creator().await.unwrap();
            task_creator = TaskCreatorEnum::Creator(creator);
        };
        let mut executor = create_executor().await.unwrap();
        let validator = Validator::new().await.unwrap();

        loop {
            let (payload, round) = task_creator.get_payload_and_round().await.unwrap();
            
            // Parse the encoded data from payload if in ingress mode
            let (target_contract, target_function, function_params) = if use_ingress {
                // Decode the payload structure:
                // [4 bytes: target_contract length][target_contract bytes]
                // [4 bytes: target_function length][target_function bytes]
                // [4 bytes: function_params length][function_params bytes]
                let mut cursor = 0;
                
                // Decode target contract
                let contract_len = u32::from_le_bytes([
                    payload[cursor], payload[cursor + 1], 
                    payload[cursor + 2], payload[cursor + 3]
                ]) as usize;
                cursor += 4;
                let target_contract = String::from_utf8_lossy(&payload[cursor..cursor + contract_len]).to_string();
                cursor += contract_len;
                
                // Decode target function
                let function_len = u32::from_le_bytes([
                    payload[cursor], payload[cursor + 1],
                    payload[cursor + 2], payload[cursor + 3]
                ]) as usize;
                cursor += 4;
                let target_function = String::from_utf8_lossy(&payload[cursor..cursor + function_len]).to_string();
                cursor += function_len;
                
                // Decode function parameters
                let params_len = u32::from_le_bytes([
                    payload[cursor], payload[cursor + 1],
                    payload[cursor + 2], payload[cursor + 3]
                ]) as usize;
                cursor += 4;
                let function_params = payload[cursor..cursor + params_len].to_vec();
                
                (target_contract, target_function, function_params)
            } else {
                // For non-ingress mode, use default values
                (
                    "0x0000000000000000000000000000000000000000".to_string(),
                    "0x00000000".to_string(),
                    vec![]
                )
            };
            
            hasher.update(&payload);
            let hashed_payload = hasher.finalize();
            info!(
                round = round.to_string(),
                msg = hex(&hashed_payload),
                target_contract = target_contract,
                target_function = target_function,
                "generated payload for round"
            );

            // Broadcast payload with target information
            let message = wire::Aggregation {
                round,
                target_contract: target_contract.clone(),
                target_function: target_function.clone(),
                function_params: function_params.clone(),
                payload: Some(Payload::Start),
            };
            let mut buf = Vec::with_capacity(message.encode_size());
            message.write(&mut buf);
            sender
                .send(commonware_p2p::Recipients::All, Bytes::from(buf), true)
                .await
                .expect("failed to broadcast message");
            signatures.insert(round, HashMap::new());
            info!(
                "Created signatures entry for round: {}, threshold is: {}",
                round, self.t
            );

            // Listen for messages until the next broadcast
            let continue_time = self.runtime.current() + self.aggregation_frequency;
            loop {
                select! {
                    _ = self.runtime.sleep_until(continue_time) => {break;},
                    msg = receiver.recv() => {
                        // Parse message
                        let (sender, msg) = match msg {
                            Ok(msg) => msg,
                            Err(_) => continue,
                        };

                        // Get contributor
                        let Some(contributor) = self.ordered_contributors.get(&sender) else {
                            info!("Received message from unknown sender: {:?}", sender);
                            continue;
                        };

                        // Check if round exists
                        let Ok(msg) = wire::Aggregation::read(&mut std::io::Cursor::new(msg)) else {
                            info!("Failed to decode message from sender: {:?}", sender);
                            continue;
                        };
                        let Some(round) = signatures.get_mut(&msg.round) else {
                            info!("Received signature for unknown round: {} from contributor: {:?}", msg.round, contributor);
                            continue;
                        };

                        // Check if contributor has already signed
                        if round.contains_key(contributor) {
                            info!("Contributor already signed for round: {} contributor: {:?}", msg.round, contributor);
                            continue;
                        }

                        // Extract signature
                        let signature = match msg.payload.clone() {
                            Some(Payload::Signature(signature)) => {
                                info!("Received signature for round: {} from contributor: {:?}", msg.round, contributor);
                                signature
                            },
                            _ => {
                                info!("Received non-signature payload from contributor: {:?}", contributor);
                                continue;
                            }
                        };
                        let Ok(signature) = Bn254Signature::try_from(signature) else {
                            info!("Failed to parse signature from contributor: {:?}", contributor);
                            continue;
                        };

                        let mut buf = Vec::with_capacity(msg.encode_size());
                        msg.write(&mut buf);
                        let expected_digest = validator.validate_and_return_expected_hash(&buf).await.unwrap();
                        info!("Verifying signature for round: {} from contributor: {:?}, expected digest: {}",
                              msg.round, contributor, hex(&expected_digest));

                        if !sender.verify(None, &expected_digest, &signature) {
                            info!("Signature verification failed for contributor: {:?}", contributor);
                            continue;
                        }

                        info!("Signature verification succeeded for contributor: {:?}", contributor);

                        // Insert signature
                        round.insert(contributor, signature);

                        // Check if should aggregate
                        info!("Current signatures count for round {}: {}, threshold: {}",
                              msg.round, round.len(), self.t);
                        if round.len() < self.t {
                            continue;
                        }

                        // Aggregate signatures
                        let mut participating = Vec::new();
                        let mut participating_g1 = Vec::new();
                        let mut signatures = Vec::new();
                        for i in 0..self.contributors.len() {
                            let Some(signature) = round.get(&i) else {
                                continue;
                            };
                            let contributor = &self.contributors[i];
                            let g1_pubkey : G1PublicKey= self.g1_map[contributor].clone();
                            participating_g1.push(g1_pubkey.clone());
                            participating.push(contributor.clone());
                            signatures.push(signature.clone());
                        }
                        let agg_signature = bn254::aggregate_signatures(&signatures).unwrap();

                        // Verify aggregated signature (already verified individual signatures so should never fail)
                        if !bn254::aggregate_verify(&participating, None, &expected_digest, &agg_signature) {
                            panic!("failed to verify aggregated signature");
                        }

                        // Execute the increment with the aggregated signature
                        match executor.execute_verification(
                            &expected_digest,
                            &participating_g1,
                            &participating,
                            &signatures,
                        ).await {
                            Ok(result) => {
                                info!(
                                    round = msg.round,
                                    "Successfully executed increment with aggregated signature. Result: {:?}",
                                    result
                                );
                            },
                            Err(e) => {
                                info!(
                                    round = msg.round,
                                    "Failed to execute increment with aggregated signature: {:?}",
                                    e
                                );
                            }
                        }
                    },
                }
            }
        }
    }
}
