-- API keys authenticating task-submission requests.
--
-- Only a keccak-256 hash of each key is stored. The raw `gk_<hex>` value is shown to the
-- operator exactly once at creation and never persisted, so a database leak cannot recover a
-- live key. `key_hash` is UNIQUE, giving an index that serves the authentication lookup.
--
-- `invalid_at` is the single timestamp that ends a key's life: NULL means it never expires, a
-- future value is a scheduled expiry set at creation, and revocation simply stamps it with the
-- current time. A key authenticates only while `invalid_at IS NULL OR invalid_at > now`, so both
-- expiry and revocation are enforced by the same check with no background sweep.
CREATE TABLE api_keys (
    id         TEXT    PRIMARY KEY NOT NULL,
    key_hash   TEXT    NOT NULL UNIQUE,
    label      TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    last_used  INTEGER,
    invalid_at INTEGER
);
