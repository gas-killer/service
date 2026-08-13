# tenop bring-up — field notes from the 2026-07-16 build-out

`scripts/tenop_runbook.sh` phases work, with four hard-won amendments:

1. **The setup Job's registration loop swallows errors** (`forge ... > /dev/null`).
   When it fails, do NOT re-run the whole setup: a fresh /main.sh pass WIPES
   `operator_keys/` in its first seconds (a 2s job-retry pod destroyed all 10
   key files here) and re-deploys a new AVS. Instead run
   `register-only-pod.yaml` (idempotent: skips operators that already have
   quorum weight, retries, visible forge output, continue-on-failure, only
   marks `.setup_complete` when all 10 registered).
   If keys are ever wiped: every ECDSA hex + BLS decimal private key is
   printed in `/root/.nodes/setup.log` at creation time — the files are
   reconstructable (`{"privateKey":...}` / `{"privateKey":...,"publicKey":<addr>}`).

2. **RegisterOperator.s.sol is not idempotent**: on a re-run the allocation
   already exists at 1e18 and `modifyAllocations` reverts `SameMagnitude()`,
   killing the script before the actual registration.
   `RegisterOperator.patched.s.sol` adds an env-gated skip (`SKIP_ALLOCATION`);
   the register pod mounts it over
   `/bls-middleware/contracts/script/RegisterOperator.s.sol` and alternates
   SKIP_ALLOCATION true/false/true across attempts (an operator whose original
   registerAsOperator failed needs the allocation pass).

3. **Fund the operators for the registration tx.** register.sh funds enough
   for staking but `registerForOperatorSets` (BLS pubkey verification) costs
   ~0.003 ETH; operators fail `insufficient funds` — drpc masks this as
   opaque HTTP-500s. Top up each testacc address with 0.005 ETH before the
   register pod. Also: use publicnode for the register RPC
   (`REGISTER_RPC_OVERRIDE`); drpc 500s eth_sendRawTransaction under load.

4. **A silently-unstaked operator shows `BelowMinimumStakeRequirement()`**:
   check `DelegationManager.operatorShares(op, lstStrategy)`; if 0, replay
   register.sh's txs with the operator key (LST `submit(0x0)` w/ 1e14 value →
   `approve` → `depositIntoStrategy` → `registerAsOperator` last, which
   self-delegates the deposit).

Deployed this run: fresh AVS registryCoordinator 0x574369b8…, wrapper
0xac91Ef6C…; consumer `GasKillerChatSharded` 0x833c59D2…, seg engine
0xEe2723cB…; 10 operators registered at weight 1e14 (op6 2e14).

## Phase-1 (prefix resume) live results — 2026-07-17

Single-slot consumer 0x16a066E8… (SDK d44f65d): settlePrefix rounds land
(21s), and a resumed ask (`prefix_len=15` of 16) ran in **369s vs 852s full
(2.3×) with a byte-identical answer**. API contract: a resume must STRICTLY
extend its warmed prefix (`prefix_len < len(prompt_ids)`); warm the fixed
template prefix, not the whole prompt. Set `GK_SIM_RPC` (publicnode) on
router AND all nodes — without it, settlement-round validation hits drpc's
broken eth_getProof and every node crash-loops. Fire warms/infers only into
a strictly stable fleet (11/11 Running sustained); segments leased to pods
that die stall until the segment timeout requeues them.
