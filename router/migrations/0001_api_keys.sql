-- API keys authenticating task-submission requests.
--
-- Only a keccak-256 hash of each key is stored. The raw `gk_<hex>` value is shown to the
-- operator exactly once at creation and never persisted, so a database leak cannot recover a
-- live key. Revocation is a soft delete — `revoked_at` is stamped rather than the row removed —
-- preserving an audit trail of issued keys. `key_hash` is UNIQUE, giving an index that serves
-- the authentication lookup.
CREATE TABLE api_keys (
    id         TEXT    PRIMARY KEY NOT NULL,
    key_hash   TEXT    NOT NULL UNIQUE,
    label      TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    last_used  INTEGER,
    revoked_at INTEGER
);
