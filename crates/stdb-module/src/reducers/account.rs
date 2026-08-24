//! Account registration, authentication, and session binding.
//!
//! An [`Account`] is the permanent email/password login. A [`Session`] is the
//! ephemeral binding from one connected `Identity` to the account that
//! authenticated it, and to whichever character (if any) it is currently
//! playing — see `tables::Session` for why the two are not the same row.
//!
//! # Password storage
//!
//! Argon2id, salted from [`ReducerContext::rng`], never a fast unsalted hash
//! and never the plaintext. The salt must come from `ctx.rng()`, never from
//! `SaltString::generate`/`OsRng`: `getrandom` fails to link on the
//! `wasm32-unknown-unknown` target this module compiles to, because
//! `spacetimedb` never registers a custom backend for it. See
//! `crates/stdb-module/Cargo.toml` for the verified-compatible dependency set.

use argon2::{Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier};
use password_hash::SaltString;
use spacetimedb::rand::RngCore;
use spacetimedb::{reducer, ReducerContext, Table};

use crate::tables::{account, session, Account, RoleRow, Session};

const MIN_PASSWORD_LEN: usize = 8;
const MAX_PASSWORD_LEN: usize = 256;

/// Generic message for every login failure. Deliberately identical whether
/// the email is unregistered or the password is wrong: telling them apart
/// would let an attacker enumerate registered emails one guess at a time.
const LOGIN_REJECTED: &str = "invalid email or password";

/// Lowercases and trims an email into its uniqueness key. Mirrors
/// `normalize_name` for the same reason: `SOMEONE@Example.com` and
/// `someone@example.com` must resolve to the same account.
pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// A minimal structural check, not full RFC 5321 validation: one `@`, with at
/// least one character on each side, a domain with a `.` in the middle, and no
/// whitespace or control characters. Good enough to reject empty strings and
/// "user@nodomain"; a full parser would still accept addresses no mail server
/// on earth would deliver to.
fn validate_email(normalized: &str) -> Result<(), String> {
    if normalized
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("email must not contain whitespace".to_string());
    }
    let Some((local, domain)) = normalized.split_once('@') else {
        return Err("email must contain exactly one '@'".to_string());
    };
    if local.is_empty() || domain.is_empty() {
        return Err("email must have text before and after '@'".to_string());
    }
    if domain.contains('@') {
        return Err("email must contain exactly one '@'".to_string());
    }
    if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
        return Err("email domain must contain a '.'".to_string());
    }
    Ok(())
}

fn validate_password(password: &str) -> Result<(), String> {
    let length = password.chars().count();
    if length < MIN_PASSWORD_LEN {
        return Err(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        ));
    }
    if length > MAX_PASSWORD_LEN {
        return Err(format!(
            "password must be at most {MAX_PASSWORD_LEN} characters"
        ));
    }
    Ok(())
}

/// Argon2id, tuned down from the crate's defaults (19 MiB, t=2) to stay well
/// inside a reducer's fuel budget. 12 MiB / t=2 / p=1 sits close to OWASP's
/// minimum recommendation rather than the strongest possible setting — watch
/// actual energy consumption on `spacetime publish` before tightening it.
fn argon2() -> Argon2<'static> {
    let params = Params::new(12 * 1024, 2, 1, None).expect("static Argon2 params are always valid");
    Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
}

/// The pure half of hashing: given salt bytes, produce a PHC string. Split
/// from [`hash_password`] so the algorithm itself — the part a mutant could
/// silently break — is unit-testable without a `ReducerContext`.
fn hash_password_with_salt(password: &str, salt_bytes: &[u8; 16]) -> Result<String, String> {
    let salt = SaltString::encode_b64(salt_bytes)
        .map_err(|_| "failed to encode password salt".to_string())?;
    argon2()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| "failed to hash password".to_string())
}

/// Hashes `password` with a fresh salt drawn from the module's deterministic
/// RNG. See the module docs for why this must never be `OsRng`.
fn hash_password(ctx: &ReducerContext, password: &str) -> Result<String, String> {
    let mut salt_bytes = [0u8; 16];
    ctx.rng().fill_bytes(&mut salt_bytes);
    hash_password_with_salt(password, &salt_bytes)
}

/// Checks `password` against a stored PHC hash. Returns `false`, never an
/// error, for a malformed stored hash — a corrupt row must fail closed, not
/// panic the reducer.
fn verify_password(hash: &str, password: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    argon2()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Creates a new account and authenticates the caller's connection as it.
///
/// Fails, without creating anything, if the normalized email is already
/// registered, if the email fails format validation, or if the password fails
/// the length policy.
#[reducer]
pub fn register(ctx: &ReducerContext, email: String, password: String) -> Result<(), String> {
    let normalized_email = normalize_email(&email);
    validate_email(&normalized_email)?;
    validate_password(&password)?;

    if ctx
        .db
        .account()
        .normalized_email()
        .find(&normalized_email)
        .is_some()
    {
        // Deliberately specific, unlike `LOGIN_REJECTED`: registration already
        // tells the caller an email is taken by construction (they just typed
        // the one being rejected), so there is nothing left to protect by
        // staying vague here.
        return Err("that email is already registered".to_string());
    }

    let password_hash = hash_password(ctx, &password)?;
    let account_row = ctx.db.account().insert(Account {
        id: 0,
        normalized_email,
        email,
        password_hash,
        role: RoleRow::Player,
        created_at: ctx.timestamp,
    });

    bind_session(ctx, account_row.id);
    crate::reducers::economy::ensure_account_economy(ctx, account_row.id);
    Ok(())
}

/// Authenticates the caller's connection as an existing account.
///
/// Returns [`LOGIN_REJECTED`] whether the email is unregistered or the
/// password is wrong, so a failed attempt cannot be used to enumerate
/// registered emails.
#[reducer]
pub fn login(ctx: &ReducerContext, email: String, password: String) -> Result<(), String> {
    let normalized_email = normalize_email(&email);
    let account_row = ctx
        .db
        .account()
        .normalized_email()
        .find(&normalized_email)
        .ok_or_else(|| LOGIN_REJECTED.to_string())?;

    if !verify_password(&account_row.password_hash, &password) {
        return Err(LOGIN_REJECTED.to_string());
    }

    bind_session(ctx, account_row.id);
    crate::reducers::economy::ensure_account_economy(ctx, account_row.id);
    Ok(())
}

/// Ends the caller's authenticated session without disconnecting. A no-op if
/// the caller was never authenticated.
#[reducer]
pub fn logout(ctx: &ReducerContext) -> Result<(), String> {
    ctx.db.session().identity().delete(ctx.sender());
    Ok(())
}

/// Resolves the caller's authenticated account, or explains why there isn't
/// one. Shared by every reducer that requires login but not necessarily a
/// selected character (e.g. `join`, which selects the character).
pub fn caller_session(ctx: &ReducerContext) -> Result<Session, String> {
    ctx.db
        .session()
        .identity()
        .find(ctx.sender())
        .ok_or_else(|| "not authenticated; call `login` or `register` first".to_string())
}

/// Creates or refreshes the caller's [`Session`] row for `account_id`.
///
/// A second `login`/`register` on the same connection re-authenticates
/// cleanly instead of failing on a duplicate primary key. `character_id` is
/// preserved only when the existing session already pointed at this same
/// account; switching accounts on one connection always returns to the
/// character list.
pub(crate) fn bind_session(ctx: &ReducerContext, account_id: u64) {
    let identity = ctx.sender();
    let character_id = ctx
        .db
        .session()
        .identity()
        .find(identity)
        .filter(|existing| existing.account_id == account_id)
        .and_then(|existing| existing.character_id);

    let row = Session {
        identity,
        account_id,
        character_id,
        authenticated_at: ctx.timestamp,
    };
    if ctx.db.session().identity().find(identity).is_some() {
        ctx.db.session().identity().update(row);
    } else {
        ctx.db.session().insert(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn salt(seed: u8) -> [u8; 16] {
        [seed; 16]
    }

    #[test]
    fn normalize_email_trims_and_lowercases() {
        assert_eq!(
            normalize_email("  Someone@Example.COM  "),
            "someone@example.com"
        );
    }

    #[test]
    fn validate_email_accepts_a_plausible_address() {
        assert!(validate_email("someone@example.com").is_ok());
    }

    #[test]
    fn validate_email_rejects_missing_at_sign() {
        assert!(validate_email("no-at-sign.com").is_err());
    }

    #[test]
    fn validate_email_rejects_empty_local_or_domain() {
        assert!(validate_email("@example.com").is_err());
        assert!(validate_email("user@").is_err());
    }

    #[test]
    fn validate_email_rejects_domain_without_dot() {
        assert!(validate_email("user@nodomain").is_err());
    }

    #[test]
    fn validate_email_rejects_multiple_at_signs() {
        assert!(validate_email("user@host@example.com").is_err());
    }

    #[test]
    fn validate_email_rejects_whitespace() {
        assert!(validate_email("us er@example.com").is_err());
    }

    #[test]
    fn validate_password_rejects_too_short() {
        assert!(validate_password("1234567").is_err()); // 7 chars
        assert!(validate_password("12345678").is_ok()); // 8 chars, the floor
    }

    #[test]
    fn validate_password_rejects_too_long() {
        assert!(validate_password(&"x".repeat(MAX_PASSWORD_LEN)).is_ok());
        assert!(validate_password(&"x".repeat(MAX_PASSWORD_LEN + 1)).is_err());
    }

    #[test]
    fn hash_password_round_trips_through_verify() {
        let hash = hash_password_with_salt("correct horse battery staple", &salt(1)).unwrap();
        assert!(verify_password(&hash, "correct horse battery staple"));
    }

    #[test]
    fn verify_password_rejects_the_wrong_password() {
        let hash = hash_password_with_salt("correct horse battery staple", &salt(1)).unwrap();
        assert!(!verify_password(&hash, "wrong password"));
    }

    #[test]
    fn hash_password_is_salted_differently_across_calls() {
        let a = hash_password_with_salt("same password", &salt(1)).unwrap();
        let b = hash_password_with_salt("same password", &salt(2)).unwrap();
        assert_ne!(
            a, b,
            "same password with different salts must hash differently"
        );
    }

    #[test]
    fn verify_password_fails_closed_on_a_malformed_hash() {
        assert!(!verify_password("not a real phc hash", "anything"));
    }
}
