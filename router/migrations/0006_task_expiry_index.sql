-- Serves the periodic TTL expiry sweep over tasks that have not reached a terminal state.
--
-- A task pins a `block_height` that goes stale as the chain advances, so one that waits past the
-- task TTL can no longer yield a payload the chain accepts. The sweep settles those rows as
-- `expired`, scanning by age within a single status. Each index is partial on that status so it
-- covers only the rows the sweep can act on — terminal rows, which dominate the table over time,
-- are absent — and each is keyed on the timestamp that status ages against:
--
--   * `queued` ages from `created_at`, the moment the request was accepted.
--   * `ready`  ages from `updated_at`, the moment aggregation recorded the payload.
--
-- SQLite only uses a partial index when the query's WHERE clause implies the index's own, so the
-- sweep must name the status as a literal rather than binding it as a parameter.
CREATE INDEX idx_tasks_ttl_queued ON tasks (created_at) WHERE status = 'queued';
CREATE INDEX idx_tasks_ttl_ready ON tasks (updated_at) WHERE status = 'ready';
