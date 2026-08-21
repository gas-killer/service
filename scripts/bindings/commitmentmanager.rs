#![allow(clippy::too_many_arguments)]
use alloy::sol;

// Minimal hand-written surface of the Commitments `CommitmentManager` (proxy) the
// deploy binary drives — declarations mirror
// `commitments/src/core/interfaces/ICommitmentManager.sol` verbatim. Hand-written
// rather than vendored ABI because the full artifact mixes namespaced
// (`ICommitmentManager.CommitmentParams`) and free-standing (`StrategyBinding`)
// struct internalTypes, which the `sol!` ABI importer cannot cross-reference.
sol! {
    #[sol(rpc)]
    contract CommitmentManager {
        struct StrategyBinding {
            address strategy;
            bytes params;
        }

        struct CommitmentParams {
            address arbiter;
            address counterparty;
            address token;
            address adapter;
            uint256 amount;
            uint16 maxPenaltyBps;
            uint64 challengeWindow;
            uint64 expiresAt;
            StrategyBinding[] strategies;
            string metadataURI;
            bytes32 metadataHash;
        }

        event CommitmentCreated(
            uint256 indexed commitmentId,
            address indexed committer,
            address indexed arbiter,
            address counterparty,
            address token,
            address adapter,
            uint256 amount,
            uint16 maxPenaltyBps,
            uint64 challengeWindow,
            uint64 expiresAt,
            string metadataURI,
            bytes32 metadataHash
        );

        function deposit(address token, uint256 amount) external;
        function createCommitment(CommitmentParams calldata params) external returns (uint256 commitmentId);
        function freeBalance(address token, address account) external view returns (uint256);
        function initiateForfeit(uint256 commitmentId, uint16 penaltyBps) external;
        function executeForfeit(uint256 commitmentId) external;
        function requestUnbond(uint256 commitmentId) external;
        function setOperatorRegistry(address registry) external;
        function operatorRegistry() external view returns (address);
    }
}
