-- Task records tracking each submitted aggregation request through its lifecycle.
--
-- A task enters in the `queued` state when a request is accepted, advances to `processing`
-- while the router aggregates operator signatures, and settles in a terminal state: `ready`
-- (carrying an executable `payload`), `failed`, or `expired` (each carrying a human-readable
-- `error`). Persisting the request parameters alongside the state lets the router rebuild and
-- re-enqueue tasks still `queued` or `processing` after a restart, so an in-flight request is
-- never silently lost when the pod recycles.
--
-- `key_id` scopes a task to the API key that created it, both for ownership checks on the
-- status endpoints and so a key's own tasks can be listed. The listing filters and paginates
-- by `(key_id, created_at)`, which the index below serves.
--
-- Ethereum values are stored as text: addresses in their `0x` hex form and `value` as a
-- decimal string, since a 256-bit integer does not fit SQLite's signed 64-bit INTEGER.
-- `call_data` holds the raw request bytes. `transition_index` is NULL when the request left
-- the slot for the server to resolve at dequeue time ("auto").
CREATE TABLE tasks (
    id               TEXT    PRIMARY KEY NOT NULL,
    key_id           TEXT    NOT NULL REFERENCES api_keys(id),
    status           TEXT    NOT NULL CHECK (status IN ('queued', 'processing', 'ready', 'failed', 'expired')),
    target_address   TEXT    NOT NULL,
    call_data        BLOB    NOT NULL,
    transition_index INTEGER,
    from_address     TEXT    NOT NULL,
    value            TEXT    NOT NULL,
    block_height     INTEGER NOT NULL,
    payload          TEXT,
    error            TEXT,
    created_at       INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at       INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Serves the per-key task listing (newest first) and its status-filtered variant.
CREATE INDEX idx_tasks_key_created ON tasks (key_id, created_at DESC);

-- Serves the startup scan that re-enqueues tasks left unfinished by a restart.
CREATE INDEX idx_tasks_status ON tasks (status);
