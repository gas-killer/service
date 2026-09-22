# sim-node

A Sepolia full node (reth + lighthouse) that serves the Gas Killer fleet's `debug_traceCall` with
the execution gas cap lifted. It is the endpoint `simRpc.url` points at in
`helm/gas-killer/testnet-overrides.yaml`, and what makes `global.simProfile=unbounded` truthful:
a hosted provider clamps `debug_traceCall` silently, and a clamped trace is indistinguishable
from a real one downstream.

The client is reth because anvil is revm too, so callTracer and prestateTracer output stays
digest-compatible with every e2e leg. geth orders callTracer logs differently, and carries a 5s
default trace timeout and a 30s HTTP write timeout that a multi-minute trace runs into.

The node is a consensus input for the fleet — every operator's `storage_updates` come from it — so
the fleet reads exactly one of these. Two endpoints fork the quorum's digests.

## Prerequisites

A node pool carrying the label and taint this chart selects on (`gaskiller.xyz/role=sim-node`,
`gaskiller.xyz/sim-node=true:NoSchedule`), sized for an 890 GB datadir and a CPU-bound trace.
It is provisioned in [gas-killer/infra](https://github.com/gas-killer/infra), not here.

## Install

```
helm upgrade --install sim-node ./helm/sim-node
```

Then seed the datadir from a snapshot — syncing Sepolia from genesis is a day of execution:

```
helm upgrade sim-node ./helm/sim-node --set replicas=0 --set snapshot.restore.enabled=true
kubectl logs -f job/sim-node-snapshot-restore      # ends with RESTORE_COMPLETE, ~90 min
helm upgrade sim-node ./helm/sim-node --set replicas=1 --set snapshot.restore.enabled=false
```

Install at `replicas=1` first even on a fresh cluster. The datadir PVC comes from the StatefulSet's
`volumeClaimTemplate`, so it exists only once pod 0 has been created, and the restore Job mounts it
by name; restoring first leaves the Job pending forever.

A StatefulSet will not roll a pod that is not Ready, so `kubectl delete pod sim-node-0` to force a
crash-looping pod onto a new revision.

## Before pointing the fleet at it

`eth_syncing` must be `false` and the head must match the chain's — until then a head-anchored
trace fails with a missing block, which is the same failure an unmoved anvil fork gives. Confirm
the cap is real in the same pass: a `2^40` `debug_traceCall` must be granted exactly
`1099511627776`, not a round number like 600,000,000.

```
kubectl exec -it sim-node-0 -c reth -- \
  cast rpc eth_syncing --rpc-url http://localhost:8545
```

Then flip the fleet with `--set simRpc.url=http://sim-node:8545`, which
`helm/gas-killer/testnet-overrides.yaml` already carries.

## Serviceability notes

- **No readiness probe, deliberately.** Readiness would pull the Service during the hours of
  initial sync. The cost is that a restarted node is back in the Service while it catches up;
  head-anchored traces fail loudly until it is current.
- **`reth.storage.size` is immutable** once the PVC exists (`volumeClaimTemplates` are), so
  growing it means editing the PVC and recreating the StatefulSet with `--cascade=orphan`.
- **The JWT secret is `helm.sh/resource-policy: keep`** and survives `helm uninstall`; delete it by
  hand if you are tearing the release down for good.
- **Snapshot archives are trusted as downloaded** — publicnode publishes no checksum. reth
  recomputes the state root for every block it syncs forward, so corruption surfaces rather than
  being served, but check the head against a trusted RPC before the fleet reads it.

## Teardown

```
helm uninstall sim-node
kubectl delete pvc reth-data-sim-node-0 lighthouse-data-sim-node-0
kubectl delete secret sim-node-jwt
```

Then fall the fleet back to the bundled anvil fork with `--set simRpc.url= --set
l1.simFork.enabled=true`, and drop the node pool in the infra repo.
