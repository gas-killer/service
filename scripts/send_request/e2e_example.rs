//! The example targets `send_request` can settle against, and how to drive each one.
//!
//! `E2E_EXAMPLE` picks which contract `run_e2e_test.sh` deploys from the manifest and which
//! transition this binary then submits against it. Every target needs the same two
//! contract-specific things — the task calldata, and the piece of state the e2e watches for change
//! to confirm the transition settled — so they live together in one enum rather than as parallel
//! `if` chains. Adding a target is a variant plus the match arms the compiler then demands,
//! instead of remembering every place that branches on the example.
//!
//! A submodule of the binary rather than part of the `scripts` library: `send_request` is the only
//! consumer, and the library is for what several binaries share. Promote it if that changes.
//!
//! The shell script maps the same `E2E_EXAMPLE` values onto manifest entry names for the deploy
//! step; that mapping stays there because `deploy_example` takes the manifest name directly.

use std::env;

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::sol_types::SolCall;

use scripts::bindings::arraysummation::ArraySummation;
use scripts::bindings::onchainlife::OnchainLife;
use scripts::bindings::reentrantcheckpoint::ReentrantCheckpoint;

/// Generations per `step` for [`E2eExample::OnchainLife`] when `ONCHAIN_LIFE_GENERATIONS` is
/// unset. At ~16.5M gas each, three puts a direct call above a 30M block while the diff it
/// produces stays at 16 board words plus the generation counter — the property the
/// unbounded-profile e2e leg asserts.
pub const DEFAULT_ONCHAIN_LIFE_GENERATIONS: u32 = 3;

/// A target the e2e settles a transition against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum E2eExample {
    /// `ArraySummation.sum(indexes)` — sums selected elements under `trackState`. Progress is
    /// `currentSum()`.
    #[default]
    ArraySummation,
    /// `ReentrantCheckpoint.advance()` — re-enters the target through an observer mid-transition,
    /// so settling it proves re-entrancy is safe under the canonical encoding. Progress is
    /// `counter()`.
    Reentrant,
    /// `OnchainLife.step(generations)` — Conway's Game of Life stepped on-chain. Several
    /// generations cost more gas than a block allows, while the resulting diff stays a fixed 17
    /// words, so this is the target for the unbounded simulation profile. Progress is
    /// `generation()`.
    OnchainLife,
}

impl E2eExample {
    /// Reads the selection from `E2E_EXAMPLE`, defaulting to [`E2eExample::ArraySummation`].
    pub fn from_env() -> Self {
        Self::parse(env::var("E2E_EXAMPLE").ok().as_deref())
    }

    /// Parses an `E2E_EXAMPLE` value (case-insensitive, trimmed). `None` / empty →
    /// `ArraySummation`.
    ///
    /// Panics on an unrecognized value rather than falling back to the default: a typo that
    /// silently selected array-summation would build calldata for a contract that isn't deployed,
    /// and the failure would surface as an opaque revert rather than a naming error.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("array-summation") => Self::ArraySummation,
            Some("reentrant") | Some("reentrant-checkpoint") => Self::Reentrant,
            Some("onchain-life") | Some("onchainlife") => Self::OnchainLife,
            Some(other) => panic!(
                "E2E_EXAMPLE must be 'array-summation', 'reentrant', or 'onchain-life', \
                 got '{other}'"
            ),
        }
    }

    /// Builds the calldata for this target's tracked function.
    ///
    /// `transition_index` is the target's current `stateTransitionCount`, which array-summation
    /// uses to sum a different slice on each trigger.
    pub fn call_data(&self, transition_index: u64) -> Vec<u8> {
        match self {
            Self::ArraySummation => {
                // Offset by 3 per trigger — [0,1,2], [3,4,5], … — so repeated runs against one
                // deployment produce different sums. The deployed array holds 100 elements.
                let base_idx = (transition_index * 3) % 97;
                let indexes = vec![
                    U256::from(base_idx),
                    U256::from(base_idx + 1),
                    U256::from(base_idx + 2),
                ];
                println!(
                    "Using ArraySummation.sum([{}, {}, {}]) for transition_index={transition_index}",
                    base_idx,
                    base_idx + 1,
                    base_idx + 2
                );
                ArraySummation::sumCall { indexes }.abi_encode().to_vec()
            }
            Self::Reentrant => {
                println!(
                    "Using ReentrantCheckpoint.advance() for transition_index={transition_index}"
                );
                ReentrantCheckpoint::advanceCall {}.abi_encode().to_vec()
            }
            Self::OnchainLife => {
                let generations = onchain_life_generations();
                println!(
                    "Using OnchainLife.step({generations}) for transition_index={transition_index}"
                );
                OnchainLife::stepCall { generations }.abi_encode().to_vec()
            }
        }
    }

    /// Reads the target's "progress" value — the state the e2e watches for change to confirm a
    /// task settled.
    pub async fn read_progress_value<P: Provider>(
        &self,
        target: Address,
        provider: &P,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        let value = match self {
            Self::ArraySummation => ArraySummation::new(target, provider)
                .currentSum()
                .call()
                .await
                .map_err(|e| format!("Failed to read currentSum(): {e}"))?,
            Self::Reentrant => ReentrantCheckpoint::new(target, provider)
                .counter()
                .call()
                .await
                .map_err(|e| format!("Failed to read counter(): {e}"))?,
            Self::OnchainLife => OnchainLife::new(target, provider)
                .generation()
                .call()
                .await
                .map_err(|e| format!("Failed to read generation(): {e}"))?,
        };
        Ok(value.to::<u64>())
    }
}

/// Generations to `step`, read from `ONCHAIN_LIFE_GENERATIONS`.
///
/// `run_e2e_test.sh` exports this and estimates the same generation count in its step 7a, so the
/// call it proves unmineable is the call submitted here. The two halves of the unbounded claim
/// only mean anything if they measure one transition, which is why the count is read rather than
/// hardcoded on both sides.
pub fn onchain_life_generations() -> u32 {
    parse_onchain_life_generations(env::var("ONCHAIN_LIFE_GENERATIONS").ok().as_deref())
}

/// Parses the `ONCHAIN_LIFE_GENERATIONS` value (trimmed). `None` / empty →
/// [`DEFAULT_ONCHAIN_LIFE_GENERATIONS`]. Panics on a non-numeric value rather than falling back:
/// silently substituting the default would settle a different transition than step 7a estimated,
/// leaving the proof measuring two different calls while still passing.
fn parse_onchain_life_generations(raw: Option<&str>) -> u32 {
    match raw.map(str::trim) {
        None | Some("") => DEFAULT_ONCHAIN_LIFE_GENERATIONS,
        Some(value) => value
            .parse()
            .unwrap_or_else(|_| panic!("ONCHAIN_LIFE_GENERATIONS must be a u32, got '{value}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_accepted_spelling() {
        assert_eq!(E2eExample::parse(None), E2eExample::ArraySummation);
        assert_eq!(E2eExample::parse(Some("")), E2eExample::ArraySummation);
        assert_eq!(
            E2eExample::parse(Some(" Array-Summation ")),
            E2eExample::ArraySummation
        );
        assert_eq!(E2eExample::parse(Some("reentrant")), E2eExample::Reentrant);
        assert_eq!(
            E2eExample::parse(Some("reentrant-checkpoint")),
            E2eExample::Reentrant
        );
        assert_eq!(
            E2eExample::parse(Some("onchain-life")),
            E2eExample::OnchainLife
        );
        assert_eq!(
            E2eExample::parse(Some("OnchainLife")),
            E2eExample::OnchainLife
        );
    }

    #[test]
    #[should_panic(expected = "E2E_EXAMPLE must be")]
    fn rejects_an_unknown_example() {
        let _ = E2eExample::parse(Some("guarded-vault"));
    }

    /// Each variant must encode its own tracked function — a mismatch would submit a call the
    /// deployed target cannot answer, which surfaces as an opaque revert rather than a clear error.
    #[test]
    fn each_example_encodes_its_own_selector() {
        assert_eq!(
            E2eExample::ArraySummation.call_data(0)[..4],
            ArraySummation::sumCall::SELECTOR
        );
        assert_eq!(
            E2eExample::Reentrant.call_data(0)[..4],
            ReentrantCheckpoint::advanceCall::SELECTOR
        );
        assert_eq!(
            E2eExample::OnchainLife.call_data(0)[..4],
            OnchainLife::stepCall::SELECTOR
        );
    }

    /// The rotating slice must stay inside the deployed array's bounds for any trigger count.
    #[test]
    fn array_summation_indexes_stay_in_bounds() {
        for transition_index in [0u64, 1, 32, 33, 1_000, u32::MAX as u64] {
            let call = ArraySummation::sumCall::abi_decode(
                &E2eExample::ArraySummation.call_data(transition_index),
            )
            .expect("array-summation calldata decodes as sum(uint256[])");
            for index in call.indexes {
                assert!(
                    index < U256::from(100),
                    "index {index} is outside the 100-element array at \
                     transition_index={transition_index}"
                );
            }
        }
    }

    #[test]
    fn onchain_life_generations_parsing() {
        assert_eq!(
            parse_onchain_life_generations(None),
            DEFAULT_ONCHAIN_LIFE_GENERATIONS
        );
        assert_eq!(
            parse_onchain_life_generations(Some("")),
            DEFAULT_ONCHAIN_LIFE_GENERATIONS
        );
        assert_eq!(
            parse_onchain_life_generations(Some("   ")),
            DEFAULT_ONCHAIN_LIFE_GENERATIONS
        );
        assert_eq!(parse_onchain_life_generations(Some("5")), 5);
        assert_eq!(parse_onchain_life_generations(Some(" 12 ")), 12);
    }

    /// A non-numeric value must not fall back to the default: step 7a estimates the count it was
    /// given while the binary would settle a different one, and the leg would pass while its two
    /// halves measured different transitions.
    #[test]
    #[should_panic(expected = "ONCHAIN_LIFE_GENERATIONS must be a u32")]
    fn onchain_life_generations_rejects_a_non_numeric_value() {
        let _ = parse_onchain_life_generations(Some("abc"));
    }
}
