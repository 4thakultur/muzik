use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

use crate::error::{internal, ApiResult, AppError};

pub const MIN_PASSWORD_LEN: usize = 8;

/// Hash used to keep timing identical when the username doesn't exist.
pub const DUMMY_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$Rf0Xn3rmXSKEq8kqHnTGbYvB0hpVbo3VSGFP2bvU0rE";

pub fn validate(password: &str) -> ApiResult<()> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(AppError::BadRequest(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if password.len() > 1024 {
        return Err(AppError::BadRequest("password is too long".into()));
    }
    Ok(())
}

/// argon2id with the crate's OWASP-recommended defaults. CPU-heavy: call from
/// `spawn_blocking`.
pub fn hash(password: &str) -> ApiResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(internal)
}

pub fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

pub async fn hash_async(password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || hash(&password))
        .await
        .map_err(internal)?
}

pub async fn verify_async(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify(&password, &hash))
        .await
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let h = hash("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify("correct horse battery", &h));
        assert!(!verify("wrong", &h));
        assert!(!verify("anything", "not a hash"));
    }

    #[test]
    fn dummy_hash_parses() {
        assert!(PasswordHash::new(DUMMY_HASH).is_ok());
    }

    #[test]
    fn length_rules() {
        assert!(validate("short").is_err());
        assert!(validate("long enough").is_ok());
    }
}
