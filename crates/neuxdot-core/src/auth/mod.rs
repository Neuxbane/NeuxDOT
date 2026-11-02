use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const JWT_SECRET: &[u8] = b"neuxdot-super-secret-production-and-dev-key-change-me";

/// Default role used when a token predates the `role` claim.
/// Such tokens are treated as the least-privileged role and a fresh token is
/// issued at the next login.
fn default_role() -> String {
    "member".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // account_id
    pub username: String,
    #[serde(default = "default_role")]
    pub role: String,
    pub exp: usize,
}

/// Minimum length for a new account password.
pub const PASSWORD_MIN_LEN: usize = 8;
/// Maximum accepted password length (guards hashing cost / abuse).
pub const PASSWORD_MAX_LEN: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PasswordPolicyError {
    #[error("Password must be at least {PASSWORD_MIN_LEN} characters long")]
    TooShort,
    #[error("Password must be at most {PASSWORD_MAX_LEN} characters long")]
    TooLong,
    #[error("Password must not contain whitespace")]
    ContainsWhitespace,
    #[error("Password must contain at least one letter")]
    MissingLetter,
    #[error("Password must contain at least one digit")]
    MissingDigit,
}

/// Validates password strength for newly set passwords.
/// Rules: 8+ chars, no whitespace, at least one letter and one digit.
pub fn validate_password_strength(password: &str) -> Result<(), PasswordPolicyError> {
    if password.len() < PASSWORD_MIN_LEN {
        return Err(PasswordPolicyError::TooShort);
    }
    if password.len() > PASSWORD_MAX_LEN {
        return Err(PasswordPolicyError::TooLong);
    }
    if password.chars().any(|c| c.is_whitespace()) {
        return Err(PasswordPolicyError::ContainsWhitespace);
    }
    if !password.chars().any(|c| c.is_alphabetic()) {
        return Err(PasswordPolicyError::MissingLetter);
    }
    if !password.chars().any(|c| c.is_numeric()) {
        return Err(PasswordPolicyError::MissingDigit);
    }
    Ok(())
}

/// A freshly generated password (e.g. for invite flows) that satisfies
/// [`validate_password_strength`].
pub fn generate_initial_password() -> String {
    loop {
        let mut candidate = generate_strong_password();
        candidate.push('a');
        candidate.push('7');
        if validate_password_strength(&candidate).is_ok() {
            return candidate;
        }
    }
}

/// Deterministically derives a 32-character root password based on the hash of the root account's created date.
/// Guarantees the password is identical on every server run for the same created date.
pub fn derive_root_password(created_at_str: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("neuxdot:root:created_at:{}", created_at_str).as_bytes());
    let hash_bytes = hasher.finalize();

    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    hash_bytes
        .iter()
        .take(32)
        .map(|&b| CHARSET[(b as usize) % CHARSET.len()] as char)
        .collect()
}

pub fn generate_strong_password() -> String {
    let rng = rand::thread_rng();
    let pass: String = rng
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    pass
}

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2.hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, hash_str: &str) -> bool {
    if let Ok(parsed_hash) = PasswordHash::new(hash_str) {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok()
    } else {
        false
    }
}

pub fn create_token(
    account_id: &str,
    username: &str,
    role: &str,
) -> Result<String, jsonwebtoken::errors::Error> {
    let expiration = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::days(30))
        .expect("valid timestamp")
        .timestamp() as usize;

    let claims = Claims {
        sub: account_id.to_string(),
        username: username.to_string(),
        role: role.to_string(),
        exp: expiration,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(JWT_SECRET),
    )
}

pub fn verify_token(token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(JWT_SECRET),
        &Validation::default(),
    )?;
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_generation_and_hashing() {
        let pass = generate_strong_password();
        assert_eq!(pass.len(), 32);

        let hash = hash_password(&pass).expect("hash success");
        assert!(verify_password(&pass, &hash));
        assert!(!verify_password("wrong-password", &hash));
    }

    #[test]
    fn test_jwt_token_roundtrip() {
        let token = create_token("acc-123", "alice", "root").expect("create token");
        let claims = verify_token(&token).expect("verify token");
        assert_eq!(claims.sub, "acc-123");
        assert_eq!(claims.username, "alice");
        assert_eq!(claims.role, "root");
    }

    #[test]
    fn test_password_strength_rules() {
        assert!(validate_password_strength("Passw0rd!").is_ok());
        assert_eq!(
            validate_password_strength("short1"),
            Err(PasswordPolicyError::TooShort)
        );
        assert_eq!(
            validate_password_strength("allletters"),
            Err(PasswordPolicyError::MissingDigit)
        );
        assert_eq!(
            validate_password_strength("1234567890"),
            Err(PasswordPolicyError::MissingLetter)
        );
        assert_eq!(
            validate_password_strength("Has Space1"),
            Err(PasswordPolicyError::ContainsWhitespace)
        );
        let generated = generate_initial_password();
        assert!(validate_password_strength(&generated).is_ok());
    }

    #[test]
    fn test_derive_root_password_determinism() {
        let date_1 = "2026-10-31 14:06:12";
        let pass_a = derive_root_password(date_1);
        let pass_b = derive_root_password(date_1);
        assert_eq!(pass_a, pass_b, "Derived password must be 100% deterministic");
        assert_eq!(pass_a.len(), 32);

        let date_2 = "2026-10-31 14:06:13";
        let pass_c = derive_root_password(date_2);
        assert_ne!(pass_a, pass_c, "Different dates must produce different passwords");
    }
}
