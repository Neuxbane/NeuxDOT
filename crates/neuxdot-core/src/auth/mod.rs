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

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // account_id
    pub username: String,
    pub exp: usize,
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

pub fn create_token(account_id: &str, username: &str) -> Result<String, jsonwebtoken::errors::Error> {
    let expiration = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::days(30))
        .expect("valid timestamp")
        .timestamp() as usize;

    let claims = Claims {
        sub: account_id.to_string(),
        username: username.to_string(),
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
        let token = create_token("acc-123", "alice").expect("create token");
        let claims = verify_token(&token).expect("verify token");
        assert_eq!(claims.sub, "acc-123");
        assert_eq!(claims.username, "alice");
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
