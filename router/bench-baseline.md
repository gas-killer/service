# Executor Performance Baseline

Snapshot p50/p95 values here after each significant testnet deploy. Use the PromQL
queries below against the in-cluster Prometheus (or Grafana Explore) over a window
that includes at least ~50 completed rounds.

## How to capture a baseline

```promql
# Replace 5m with whatever window gives you a stable rate (10m+ preferred)
histogram_quantile(0.50, rate(gas_killer_executor_chain_detection_seconds_bucket[5m]))
histogram_quantile(0.95, rate(gas_killer_executor_chain_detection_seconds_bucket[5m]))

histogram_quantile(0.50, rate(gas_killer_executor_hash_preflight_seconds_bucket[5m]))
histogram_quantile(0.95, rate(gas_killer_executor_hash_preflight_seconds_bucket[5m]))

histogram_quantile(0.50, rate(gas_killer_executor_supports_interface_seconds_bucket[5m]))
histogram_quantile(0.95, rate(gas_killer_executor_supports_interface_seconds_bucket[5m]))

histogram_quantile(0.50, rate(gas_killer_executor_tx_send_seconds_bucket[5m]))
histogram_quantile(0.95, rate(gas_killer_executor_tx_send_seconds_bucket[5m]))

histogram_quantile(0.50, rate(gas_killer_executor_receipt_confirmation_seconds_bucket[5m]))
histogram_quantile(0.95, rate(gas_killer_executor_receipt_confirmation_seconds_bucket[5m]))

histogram_quantile(0.50, rate(gas_killer_execution_duration_seconds_bucket[5m]))
histogram_quantile(0.95, rate(gas_killer_execution_duration_seconds_bucket[5m]))

histogram_quantile(0.50, rate(gas_killer_p2p_round_trip_seconds_bucket[5m]))
histogram_quantile(0.99, rate(gas_killer_p2p_round_trip_seconds_bucket[5m]))
```

## Snapshots

| Date | Commit | chain_detection p50/p95 | hash_preflight p50/p95 | supports_interface p50/p95 | tx_send p50/p95 | receipt_confirmation p50/p95 | execution_duration p50/p95 | p2p_round_trip p50/p99 | Notes |
|------|--------|------------------------|------------------------|---------------------------|-----------------|------------------------------|---------------------------|------------------------|-------|
| 02/06/2026 | [d4ba093](https://github.com/gas-killer/service/commit/d4ba093511f6e49ae2c98efad247389beb960051) | 40/49ms | 25/29.5ms | 15.0/19.5ms | 75.0/97.5ms | 8.0/14.7s | 8.0/15.6s | 1.50/1.99s | First deploy with per-phase metrics |

## Aggregation speed

End-to-end throughput in **rounds per minute** — each round is one successful, on-chain
`verifyAndUpdate`. Same window guidance as above (10m+ preferred for a stable rate).

```promql
# Successful aggregation rounds per minute
sum(rate(gas_killer_aggregation_rounds_completed_total[5m])) * 60

# Failed rounds per minute (context)
sum(rate(gas_killer_aggregation_rounds_failed_total[5m])) * 60
```

| Date | Commit | aggregation_speed (rounds/min) | Notes |
|------|--------|--------------------------------|-------|
| 02/06/2026 | [d4ba093](https://github.com/gas-killer/service/commit/d4ba093511f6e49ae2c98efad247389beb960051) | 0.571 | - |
