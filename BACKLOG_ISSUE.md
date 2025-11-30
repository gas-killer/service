# GitHub Issue: Architecture Refactor Proposal

**Repo:** BreadchainCoop/commonware-restaking
**Title:** Architecture: Consider passing validator to creator instead of orchestrator

---

## Context

In the gas-killer-router implementation, we noticed that the `Orchestrator` type requires a validator as a type parameter:

```rust
pub type GasKillerOrchestrator<C> = Orchestrator<
    GasKillerCreatorType,
    BlsEigenlayerExecutor<GasKillerHandler>,
    GasKillerValidator,
    C,
>;
```

However, looking at the architecture:
1. **Creator** needs to compute values (like storage updates) when creating tasks
2. **Nodes** independently validate before signing
3. **Orchestrator** aggregates signatures and submits

## Question

Does the orchestrator actually use the validator? If validation is handled by nodes independently, the orchestrator may not need a validator at all.

## Suggested Refactor

Consider whether the `OrchestratorBuilder::build()` should accept the validator for the **creator** rather than the orchestrator itself. This would:
- Make the dependency clearer (creator is the one that needs validation/computation capabilities)
- Allow the orchestrator to focus on aggregation and submission
- Simplify the type signature if the orchestrator doesn't actually use the validator

## Current Workaround

For now, we're sharing an `Arc<GasKillerValidator>` between the creator and orchestrator to avoid duplicating the validator instance.
