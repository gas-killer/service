//! API key issuance, revocation, and verification backed by the [`SqliteStore`].
//!
//! A key is an opaque `gk_<64 hex>` string: a 4-character prefix for easy identification in
//! logs plus 32 bytes of cryptographically secure randomness. Only the keccak-256 hash of the
//! key is stored, so the raw value exists only in the response to the create call and in the
//! caller's possession — a database leak cannot recover a usable key.
//!
//! Authentication looks a key up by its hash. keccak-256 is preimage-resistant and the key
//! carries 256 bits of entropy, so this is not vulnerable to the timing attacks that a
//! byte-wise comparison of the raw secret would invite: an attacker cannot use lookup timing to
//! recover the key, and the raw secret is never compared directly.

use anyhow::Context;
use rand::RngCore;
use serde::Serialize;

use super::SqliteStore;

/// Prefix identifying Gas Killer API keys in logs and client configuration.
const KEY_PREFIX: &str = "gk_";

/// Number of random bytes in the secret portion of a key. 32 bytes (256 bits) makes both
/// guessing and hash-collision attacks infeasible.
const KEY_BYTES: usize = 32;

/// Number of random bytes in a key's public identifier, used in URLs and listings.
const ID_BYTES: usize = 8;

/// A newly created API key, including the raw secret. The `key` is returned to the caller
/// exactly once — it is never persisted in the clear and cannot be recovered afterwards.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CreatedApiKey {
    pub id: String,
    pub key: String,
    pub label: Option<String>,
    pub created_at: i64,
    /// Unix timestamp at which the key stops authenticating, or `None` if it never expires.
    pub invalid_at: Option<i64>,
    /// Per-key requests-per-minute override for `POST /tasks`, or `None` to use the global
    /// default rate.
    pub rpm_limit: Option<u32>,
}

/// Non-secret metadata about an active API key, safe to list. Deliberately omits the key value
/// and its hash so neither is ever exposed through the admin API.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ApiKeyMetadata {
    pub id: String,
    pub label: Option<String>,
    pub created_at: i64,
    pub last_used: Option<i64>,
    /// Unix timestamp at which the key expires, or `None` if it never expires. Listed keys are
    /// always still valid, so this is only ever null or a future time.
    pub invalid_at: Option<i64>,
    /// Per-key requests-per-minute override, or `None` when the key uses the global default rate.
    pub rpm_limit: Option<u32>,
}

/// A key that authenticates: its public id and its rate-limit ceiling. Carried by
/// [`KeyVerdict::Valid`] so the ingress can both attribute the request to a key and pick the right
/// rate-limit quota from one lookup.
#[derive(Debug, Clone)]
pub struct AuthenticatedKey {
    pub id: String,
    /// Per-key requests-per-minute override, or `None` when the key uses the global default rate.
    pub rpm_limit: Option<u32>,
}

/// What [`SqliteStore::verify_api_key`] made of a presented key value.
///
/// The two rejection variants are distinct because a request is only auditable when it can be
/// attributed: a key this router issued and has since revoked or expired is still nameable, which
/// is exactly the traffic worth investigating, while a value that was never a key has no id to
/// name. Both are the same `401` to the caller.
#[derive(Debug, Clone)]
pub enum KeyVerdict {
    /// The key is valid; `last_used` has been stamped.
    Valid(AuthenticatedKey),
    /// A key this router issued that no longer authenticates — revoked or past its expiry — named
    /// by its public id so the rejection can be attributed. `last_used` is left untouched, so a
    /// rejected request never registers as a use of the key.
    Rejected { id: String },
    /// The presented value matches no key this router ever issued.
    Unknown,
}

/// Generates a fresh opaque key: `gk_` followed by 32 hex-encoded random bytes.
fn generate_key() -> String {
    let mut bytes = [0u8; KEY_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    format!("{KEY_PREFIX}{}", hex::encode(bytes))
}

/// Generates a random public identifier for a key.
fn generate_id() -> String {
    let mut bytes = [0u8; ID_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Hashes a raw key for storage and lookup. keccak-256 is preimage-resistant, so the stored
/// digest cannot be reversed to the key.
fn hash_key(raw: &str) -> String {
    hex::encode(alloy_primitives::keccak256(raw.as_bytes()))
}

impl SqliteStore {
    /// Issues a new API key limited at the global default rate. Convenience wrapper over
    /// [`create_api_key_with_rpm`](Self::create_api_key_with_rpm) with no per-key override.
    pub async fn create_api_key(
        &self,
        label: Option<String>,
        invalid_at: Option<i64>,
    ) -> anyhow::Result<CreatedApiKey> {
        self.create_api_key_with_rpm(label, invalid_at, None).await
    }

    /// Issues a new API key with an optional human-readable label, optional expiry (`invalid_at`,
    /// a unix timestamp; `None` never expires), and an optional per-key requests-per-minute
    /// override (`None` uses the global default rate), persisting only its hash. The returned
    /// [`CreatedApiKey`] carries the raw key value, which the caller must surface to the operator
    /// immediately — it cannot be retrieved again.
    pub async fn create_api_key_with_rpm(
        &self,
        label: Option<String>,
        invalid_at: Option<i64>,
        rpm_limit: Option<u32>,
    ) -> anyhow::Result<CreatedApiKey> {
        let key = generate_key();
        let id = generate_id();
        let key_hash = hash_key(&key);

        let created_at: i64 = sqlx::query_scalar(
            "INSERT INTO api_keys (id, key_hash, label, invalid_at, rpm_limit) \
             VALUES (?1, ?2, ?3, ?4, ?5) RETURNING created_at",
        )
        .bind(&id)
        .bind(&key_hash)
        .bind(label.as_deref())
        .bind(invalid_at)
        .bind(rpm_limit)
        .fetch_one(self.pool())
        .await
        .context("inserting api key")?;

        Ok(CreatedApiKey {
            id,
            key,
            label,
            created_at,
            invalid_at,
            rpm_limit,
        })
    }

    /// Lists metadata for every still-valid key (neither revoked nor expired), most recently
    /// created first. The key values and hashes are never returned.
    pub async fn list_api_keys(&self) -> anyhow::Result<Vec<ApiKeyMetadata>> {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                Option<String>,
                i64,
                Option<i64>,
                Option<i64>,
                Option<u32>,
            ),
        >(
            "SELECT id, label, created_at, last_used, invalid_at, rpm_limit FROM api_keys \
             WHERE invalid_at IS NULL OR invalid_at > unixepoch() ORDER BY created_at DESC, id",
        )
        .fetch_all(self.pool())
        .await
        .context("listing api keys")?;

        Ok(rows
            .into_iter()
            .map(
                |(id, label, created_at, last_used, invalid_at, rpm_limit)| ApiKeyMetadata {
                    id,
                    label,
                    created_at,
                    last_used,
                    invalid_at,
                    rpm_limit,
                },
            )
            .collect())
    }

    /// Revokes the key with the given id, taking effect immediately by stamping `invalid_at`
    /// with the current time (overriding any later scheduled expiry). Returns `true` if a
    /// currently-valid key was revoked, `false` if no such key exists (already revoked, already
    /// expired, or never issued).
    pub async fn revoke_api_key(&self, id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE api_keys SET invalid_at = unixepoch() \
             WHERE id = ?1 AND (invalid_at IS NULL OR invalid_at > unixepoch())",
        )
        .bind(id)
        .execute(self.pool())
        .await
        .context("revoking api key")?;

        Ok(result.rows_affected() > 0)
    }

    /// Authenticates a presented key, returning a [`KeyVerdict`]: valid (with the id and
    /// rate-limit ceiling), rejected but nameable, or unknown. Lookup is by hash, so the raw
    /// secret is never compared byte-wise.
    ///
    /// One statement decides all three outcomes. Matching on `key_hash` alone means a revoked or
    /// expired key still yields its row — that is what makes a rejection attributable — so the
    /// validity test moves into the `SET`, stamping `last_used` only for a key that actually
    /// authenticates, and comes back alongside the id as a flag. `invalid_at` is not assigned, so
    /// the flag reads the same before and after the update.
    pub async fn verify_api_key(&self, presented: &str) -> anyhow::Result<KeyVerdict> {
        let key_hash = hash_key(presented);

        let row: Option<(String, Option<u32>, bool)> = sqlx::query_as(
            "UPDATE api_keys \
                SET last_used = CASE WHEN invalid_at IS NULL OR invalid_at > unixepoch() \
                                     THEN unixepoch() ELSE last_used END \
              WHERE key_hash = ?1 \
             RETURNING id, rpm_limit, (invalid_at IS NULL OR invalid_at > unixepoch())",
        )
        .bind(&key_hash)
        .fetch_optional(self.pool())
        .await
        .context("verifying api key")?;

        Ok(match row {
            Some((id, rpm_limit, true)) => KeyVerdict::Valid(AuthenticatedKey { id, rpm_limit }),
            Some((id, _, false)) => KeyVerdict::Rejected { id },
            None => KeyVerdict::Unknown,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> SqliteStore {
        SqliteStore::connect_in_memory()
            .await
            .expect("in-memory store should open and migrate")
    }

    /// The authenticated key behind a [`KeyVerdict::Valid`], or a panic naming what came back.
    fn expect_valid(verdict: KeyVerdict) -> AuthenticatedKey {
        match verdict {
            KeyVerdict::Valid(authed) => authed,
            other => panic!("expected a valid key, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn created_key_has_expected_shape() {
        let store = store().await;
        let created = store
            .create_api_key(Some("client-a".to_string()), None)
            .await
            .expect("key creation should succeed");

        assert!(
            created.key.starts_with("gk_"),
            "key should carry the prefix"
        );
        // gk_ + 32 bytes hex-encoded (64 chars).
        assert_eq!(created.key.len(), KEY_PREFIX.len() + KEY_BYTES * 2);
        assert_eq!(created.label.as_deref(), Some("client-a"));
        assert!(created.created_at > 0, "created_at should be stamped");
        assert!(!created.id.is_empty());
        assert_eq!(created.invalid_at, None, "no expiry was requested");
    }

    #[tokio::test]
    async fn each_key_is_unique() {
        let store = store().await;
        let a = store.create_api_key(None, None).await.unwrap();
        let b = store.create_api_key(None, None).await.unwrap();
        assert_ne!(a.key, b.key);
        assert_ne!(a.id, b.id);
    }

    #[tokio::test]
    async fn verify_accepts_valid_key_and_stamps_last_used() {
        let store = store().await;
        let created = store.create_api_key(None, None).await.unwrap();

        let authed = expect_valid(
            store
                .verify_api_key(&created.key)
                .await
                .expect("verify should not error"),
        );
        assert_eq!(authed.id, created.id);

        // last_used starts null and is set after a successful verification.
        let listed = store.list_api_keys().await.unwrap();
        let entry = listed
            .iter()
            .find(|k| k.id == created.id)
            .expect("created key should be listed");
        assert!(
            entry.last_used.is_some(),
            "verifying a key should stamp last_used"
        );
    }

    #[tokio::test]
    async fn verify_rejects_unknown_key() {
        let store = store().await;
        store.create_api_key(None, None).await.unwrap();

        let verdict = store
            .verify_api_key("gk_deadbeef")
            .await
            .expect("verify should not error");
        assert!(
            matches!(verdict, KeyVerdict::Unknown),
            "a value that was never a key must not authenticate and has no id to name, got \
             {verdict:?}"
        );
    }

    #[tokio::test]
    async fn revoked_key_no_longer_authenticates() {
        let store = store().await;
        let created = store.create_api_key(None, None).await.unwrap();

        assert!(
            store.revoke_api_key(&created.id).await.unwrap(),
            "revoking an active key should report success"
        );
        assert!(
            matches!(
                store.verify_api_key(&created.key).await.unwrap(),
                KeyVerdict::Rejected { .. }
            ),
            "a revoked key must not authenticate"
        );
    }

    #[tokio::test]
    async fn revoke_is_not_idempotent_success() {
        let store = store().await;
        let created = store.create_api_key(None, None).await.unwrap();

        assert!(store.revoke_api_key(&created.id).await.unwrap());
        assert!(
            !store.revoke_api_key(&created.id).await.unwrap(),
            "revoking an already-revoked key should report no change"
        );
        assert!(
            !store.revoke_api_key("does-not-exist").await.unwrap(),
            "revoking an unknown id should report no change"
        );
    }

    #[tokio::test]
    async fn list_excludes_revoked_keys() {
        let store = store().await;
        let keep = store
            .create_api_key(Some("keep".to_string()), None)
            .await
            .unwrap();
        let drop = store
            .create_api_key(Some("drop".to_string()), None)
            .await
            .unwrap();

        store.revoke_api_key(&drop.id).await.unwrap();

        let listed = store.list_api_keys().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, keep.id);
        assert_eq!(listed[0].label.as_deref(), Some("keep"));
    }

    // A timestamp far in the future ("year 2100"), used to exercise a key that carries an expiry
    // but has not lapsed.
    const FUTURE: i64 = 4_102_444_800;

    #[tokio::test]
    async fn key_with_future_expiry_authenticates_and_lists() {
        let store = store().await;
        let created = store.create_api_key(None, Some(FUTURE)).await.unwrap();
        assert_eq!(created.invalid_at, Some(FUTURE));

        assert!(
            matches!(
                store.verify_api_key(&created.key).await.unwrap(),
                KeyVerdict::Valid(_)
            ),
            "a key whose expiry is in the future should authenticate"
        );

        let listed = store.list_api_keys().await.unwrap();
        let entry = listed
            .iter()
            .find(|k| k.id == created.id)
            .expect("an unexpired key should be listed");
        assert_eq!(
            entry.invalid_at,
            Some(FUTURE),
            "listing should surface expiry"
        );
    }

    #[tokio::test]
    async fn expired_key_is_rejected_and_unlisted() {
        let store = store().await;
        // An expiry in the distant past (1970) is already lapsed at creation.
        let created = store.create_api_key(None, Some(1)).await.unwrap();

        assert!(
            matches!(
                store.verify_api_key(&created.key).await.unwrap(),
                KeyVerdict::Rejected { .. }
            ),
            "a key past its expiry must not authenticate"
        );
        assert!(
            store.list_api_keys().await.unwrap().is_empty(),
            "an expired key must not appear in the active listing"
        );
    }

    #[tokio::test]
    async fn revoking_a_future_expiry_key_invalidates_it_immediately() {
        let store = store().await;
        let created = store.create_api_key(None, Some(FUTURE)).await.unwrap();

        assert!(
            store.revoke_api_key(&created.id).await.unwrap(),
            "revoking a key with a pending expiry should report success"
        );
        assert!(
            matches!(
                store.verify_api_key(&created.key).await.unwrap(),
                KeyVerdict::Rejected { .. }
            ),
            "revocation must invalidate the key ahead of its scheduled expiry"
        );
    }

    #[tokio::test]
    async fn rpm_override_round_trips_through_create_verify_and_list() {
        let store = store().await;
        let created = store
            .create_api_key_with_rpm(Some("fast".to_string()), None, Some(600))
            .await
            .unwrap();
        assert_eq!(created.rpm_limit, Some(600));

        let authed = expect_valid(store.verify_api_key(&created.key).await.unwrap());
        assert_eq!(authed.id, created.id);
        assert_eq!(
            authed.rpm_limit,
            Some(600),
            "verify should surface the per-key override"
        );

        let listed = store.list_api_keys().await.unwrap();
        let entry = listed.iter().find(|k| k.id == created.id).unwrap();
        assert_eq!(entry.rpm_limit, Some(600));
    }

    #[tokio::test]
    async fn key_without_override_reports_no_rpm_limit() {
        let store = store().await;
        let created = store.create_api_key(None, None).await.unwrap();
        assert_eq!(created.rpm_limit, None);

        let authed = expect_valid(store.verify_api_key(&created.key).await.unwrap());
        assert_eq!(
            authed.rpm_limit, None,
            "a key with no override falls back to the global default rate"
        );
    }

    #[tokio::test]
    async fn a_key_that_no_longer_authenticates_is_still_named() {
        let store = store().await;
        let revoked = store.create_api_key(None, None).await.unwrap();
        store.revoke_api_key(&revoked.id).await.unwrap();
        // An expiry in the distant past (1970) is already lapsed at creation.
        let expired = store.create_api_key(None, Some(1)).await.unwrap();

        // Neither key authenticates any more, but both remain attributable, so a rejected request
        // can still name the client that sent it.
        for created in [&revoked, &expired] {
            match store.verify_api_key(&created.key).await.unwrap() {
                KeyVerdict::Rejected { id } => assert_eq!(id, created.id),
                other => panic!("expected a named rejection, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_rejected_key_does_not_have_its_last_used_stamped() {
        let store = store().await;
        let created = store.create_api_key(None, None).await.unwrap();
        store.revoke_api_key(&created.id).await.unwrap();

        // Plant a sentinel `last_used` rather than authenticating once for a real stamp: a real
        // stamp and an unwanted re-stamp fall in the same second, so they are indistinguishable.
        // A value no clock would produce makes an overwrite unmistakable.
        const SENTINEL: i64 = 12_345;
        sqlx::query("UPDATE api_keys SET last_used = ?1 WHERE id = ?2")
            .bind(SENTINEL)
            .bind(&created.id)
            .execute(store.pool())
            .await
            .unwrap();

        // Naming the key for the audit log must not register as a use of it: the one statement
        // stamps `last_used` only on the branch that actually authenticates.
        for _ in 0..3 {
            assert!(matches!(
                store.verify_api_key(&created.key).await.unwrap(),
                KeyVerdict::Rejected { .. }
            ));
        }

        // A revoked key drops out of the active listing, so read the column directly.
        let last_used: Option<i64> =
            sqlx::query_scalar("SELECT last_used FROM api_keys WHERE id = ?1")
                .bind(&created.id)
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(
            last_used,
            Some(SENTINEL),
            "a rejected request must leave last_used where the last real use left it"
        );
    }

    #[tokio::test]
    async fn a_value_never_issued_has_no_id_to_name() {
        let store = store().await;
        store.create_api_key(None, None).await.unwrap();

        assert!(
            matches!(
                store.verify_api_key("gk_deadbeef").await.unwrap(),
                KeyVerdict::Unknown
            ),
            "a value that matches no issued key has no id to attribute a request to"
        );
    }

    #[test]
    fn hash_is_deterministic_and_input_dependent() {
        assert_eq!(hash_key("gk_abc"), hash_key("gk_abc"));
        assert_ne!(hash_key("gk_abc"), hash_key("gk_abd"));
        // keccak-256 digest, hex-encoded, is 64 characters.
        assert_eq!(hash_key("gk_abc").len(), 64);
    }
}
