//! Account-scoped HTTP API keys.
//!
//! The plaintext secret is minted by the gateway (it has OS entropy) and
//! handed to [`create_api_key`], which stores only a SHA-256 hex digest.
//! Listing goes through the `my_api_keys` view so clients never see hashes.
//! Authentication for a bot is [`authenticate_api_key`], which binds a
//! [`crate::tables::Session`] the same way `login` does.

use sha2::{Digest, Sha256};
use spacetimedb::{reducer, view, ReducerContext, Table, Uuid, ViewContext};

use crate::reducers::account::{bind_session, caller_session};
use crate::tables::{api_key, api_key__view, session__view, ApiKey, ApiKeyMeta};
use crate::MAX_API_KEYS_PER_ACCOUNT;

/// Matches the gateway's mint: `eiv_` plus 32 random bytes as lowercase hex.
pub const SECRET_PREFIX: &str = "eiv_";
const SECRET_HEX_LEN: usize = 64;
const SECRET_LEN: usize = SECRET_PREFIX.len() + SECRET_HEX_LEN;
/// `eiv_` + first 8 hex chars — enough to tell keys apart, not enough to brute.
pub const DISPLAY_PREFIX_LEN: usize = SECRET_PREFIX.len() + 8;

const MIN_NAME_LEN: usize = 1;
const MAX_NAME_LEN: usize = 48;

/// Generic message for every authenticate failure. Same reason as login:
/// distinguishing "unknown" from "malformed" would let an attacker probe.
const INVALID_API_KEY: &str = "invalid api key";

/// SHA-256 of `secret`, lowercase hex. Pure so the algorithm is unit-testable
/// without a `ReducerContext`.
pub fn hash_api_key(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    hex_encode(&digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn is_lowercase_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Trimmed name, 1–48 chars, no control characters.
pub fn validate_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    let length = trimmed.chars().count();
    if !(MIN_NAME_LEN..=MAX_NAME_LEN).contains(&length) {
        return Err(format!(
            "name must be between {MIN_NAME_LEN} and {MAX_NAME_LEN} characters"
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err("name must not contain control characters".to_string());
    }
    Ok(trimmed.to_string())
}

/// The secret the gateway mints: `eiv_` + 64 lowercase hex chars.
pub fn validate_secret(secret: &str) -> Result<(), String> {
    let Some(hex) = secret.strip_prefix(SECRET_PREFIX) else {
        return Err("api key must start with eiv_".to_string());
    };
    if secret.len() != SECRET_LEN || hex.len() != SECRET_HEX_LEN || !is_lowercase_hex(hex) {
        return Err("api key must be eiv_ followed by 64 hex characters".to_string());
    }
    Ok(())
}

/// `prefix` is the first [`DISPLAY_PREFIX_LEN`] characters of `secret`.
pub fn validate_prefix(secret: &str, prefix: &str) -> Result<(), String> {
    let expected: String = secret.chars().take(DISPLAY_PREFIX_LEN).collect();
    if prefix != expected {
        return Err("prefix must be the first 12 characters of the api key".to_string());
    }
    Ok(())
}

/// Creates a new API key for the caller's account. The gateway chooses `id`
/// and `secret` so the HTTP response can return the plaintext without the
/// reducer needing a return value.
#[reducer]
pub fn create_api_key(
    ctx: &ReducerContext,
    id: Uuid,
    name: String,
    prefix: String,
    secret: String,
) -> Result<(), String> {
    let session = caller_session(ctx)?;
    let name = validate_name(&name)?;
    validate_secret(&secret)?;
    validate_prefix(&secret, &prefix)?;

    if ctx.db.api_key().id().find(id).is_some() {
        return Err("api key id already exists".to_string());
    }

    let existing = ctx
        .db
        .api_key()
        .account_id()
        .filter(&session.account_id)
        .count();
    if existing >= MAX_API_KEYS_PER_ACCOUNT {
        return Err(format!(
            "at most {MAX_API_KEYS_PER_ACCOUNT} api keys per account"
        ));
    }

    let key_hash = hash_api_key(&secret);
    if ctx.db.api_key().key_hash().find(&key_hash).is_some() {
        return Err("api key already exists".to_string());
    }

    ctx.db.api_key().insert(ApiKey {
        id,
        account_id: session.account_id,
        key_hash,
        name,
        prefix,
        created_at: ctx.timestamp,
        last_used_at: None,
    });
    Ok(())
}

/// Deletes one of the caller's API keys. Unknown ids and other accounts'
/// keys are distinct errors so the website can show 404 vs 403.
#[reducer]
pub fn revoke_api_key(ctx: &ReducerContext, id: Uuid) -> Result<(), String> {
    let session = caller_session(ctx)?;
    let Some(row) = ctx.db.api_key().id().find(id) else {
        return Err("no api key with that id".to_string());
    };
    if row.account_id != session.account_id {
        return Err("that api key does not belong to this account".to_string());
    }
    ctx.db.api_key().id().delete(id);
    Ok(())
}

/// Authenticates this connection as the account that owns `secret`, the
/// same way `login` does. Updates `last_used_at`.
#[reducer]
pub fn authenticate_api_key(ctx: &ReducerContext, secret: String) -> Result<(), String> {
    if validate_secret(&secret).is_err() {
        return Err(INVALID_API_KEY.to_string());
    }
    let hash = hash_api_key(&secret);
    let Some(row) = ctx.db.api_key().key_hash().find(&hash) else {
        return Err(INVALID_API_KEY.to_string());
    };

    let updated = ApiKey {
        last_used_at: Some(ctx.timestamp),
        ..row
    };
    let account_id = updated.account_id;
    ctx.db.api_key().id().update(updated);
    bind_session(ctx, account_id);
    Ok(())
}

/// The caller's own API keys, without hashes. Empty when the connection has
/// not authenticated. Indexed lookups only — views cannot scan.
#[view(accessor = my_api_keys, public, primary_key = id)]
fn my_api_keys(ctx: &ViewContext) -> Vec<ApiKeyMeta> {
    let Some(session_row) = ctx.db.session().identity().find(ctx.sender()) else {
        return Vec::new();
    };
    ctx.db
        .api_key()
        .account_id()
        .filter(&session_row.account_id)
        .map(|row| ApiKeyMeta {
            id: row.id,
            name: row.name,
            prefix: row.prefix,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_secret(fill: u8) -> String {
        format!("{SECRET_PREFIX}{}", format!("{fill:02x}").repeat(32))
    }

    #[test]
    fn valid_secret_helper_has_the_documented_shape() {
        let secret = valid_secret(0xab);
        assert_eq!(secret.len(), SECRET_LEN);
        assert!(secret.starts_with(SECRET_PREFIX));
        assert!(validate_secret(&secret).is_ok());
    }

    #[test]
    fn hash_api_key_is_deterministic() {
        let secret = valid_secret(1);
        assert_eq!(hash_api_key(&secret), hash_api_key(&secret));
        assert_eq!(hash_api_key(&secret).len(), 64);
        assert!(is_lowercase_hex(&hash_api_key(&secret)));
    }

    #[test]
    fn hash_api_key_differs_across_secrets() {
        assert_ne!(
            hash_api_key(&valid_secret(1)),
            hash_api_key(&valid_secret(2))
        );
    }

    #[test]
    fn hash_api_key_matches_the_gateway_vector() {
        // Keep in lockstep with `apps/gateway` `hash_api_key_matches_the_module_vector`.
        let secret = format!("{SECRET_PREFIX}{}", "ab".repeat(32));
        assert_eq!(
            hash_api_key(&secret),
            "7e264f0416dc383541f0ac3088053aa9f77edcc65371e181fc2a735aa916b7c4"
        );
    }

    #[test]
    fn validate_name_trims_and_accepts_a_label() {
        assert_eq!(validate_name("  discord-bot  ").unwrap(), "discord-bot");
    }

    #[test]
    fn validate_name_rejects_empty_or_whitespace() {
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
    }

    #[test]
    fn validate_name_rejects_too_long() {
        assert!(validate_name(&"x".repeat(MAX_NAME_LEN)).is_ok());
        assert!(validate_name(&"x".repeat(MAX_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn validate_name_rejects_control_characters() {
        assert!(validate_name("bad\nname").is_err());
    }

    #[test]
    fn validate_secret_rejects_wrong_prefix_or_length() {
        assert!(validate_secret("ghp_not_ours").is_err());
        assert!(validate_secret("eiv_short").is_err());
        assert!(validate_secret(&format!("eiv_{}", "G".repeat(64))).is_err());
        assert!(validate_secret(&valid_secret(0xaa)).is_ok());
    }

    #[test]
    fn validate_prefix_must_be_the_start_of_the_secret() {
        let secret = valid_secret(0xcd);
        let prefix: String = secret.chars().take(DISPLAY_PREFIX_LEN).collect();
        assert!(validate_prefix(&secret, &prefix).is_ok());
        assert!(validate_prefix(&secret, "eiv_ffffffff").is_err());
        assert_eq!(prefix.len(), DISPLAY_PREFIX_LEN);
        assert!(prefix.starts_with(SECRET_PREFIX));
    }
}
