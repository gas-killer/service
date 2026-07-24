-- Per-key request rate-limit override for the ingress POST /tasks endpoint.
--
-- NULL means the key is limited at the global default (RATE_LIMIT_RPM); a positive integer is a
-- per-key requests-per-minute override chosen at creation time. The rate-limit counters
-- themselves are in-memory and reset on restart — only this ceiling is persisted, so a restart
-- rebuilds each key's limiter from its stored quota.
ALTER TABLE api_keys ADD COLUMN rpm_limit INTEGER;
