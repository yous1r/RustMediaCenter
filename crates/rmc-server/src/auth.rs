use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
}

const DEFAULT_ADMIN_USERNAME: &str = "admin";
const DEFAULT_ADMIN_PASSWORD: &str = "admin";
const DEFAULT_JWT_SECRET: &str = "change-me-in-production";

pub fn admin_username() -> String {
    std::env::var("RMC_ADMIN_USERNAME").unwrap_or_else(|_| DEFAULT_ADMIN_USERNAME.to_string())
}

pub fn admin_password() -> String {
    std::env::var("RMC_ADMIN_PASSWORD").unwrap_or_else(|_| DEFAULT_ADMIN_PASSWORD.to_string())
}

pub fn jwt_secret() -> String {
    std::env::var("RMC_JWT_SECRET").unwrap_or_else(|_| DEFAULT_JWT_SECRET.to_string())
}

pub fn validate_admin_login(username: &str, password: &str) -> bool {
    username == admin_username() && password == admin_password()
}

pub fn create_jwt(username: &str) -> Result<String, jsonwebtoken::errors::Error> {
    let expiration = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::hours(24))
        .expect("valid timestamp")
        .timestamp() as usize;

    let claims = Claims {
        sub: username.to_owned(),
        exp: expiration,
    };

    let secret = jwt_secret();
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

pub fn verify_jwt(token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let secret = jwt_secret();
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    Ok(token_data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        entries: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn set(entries: &[(&'static str, &'static str)]) -> Self {
            let lock = ENV_MUTEX.lock().unwrap();
            let mut previous = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                previous.push((*key, std::env::var(key).ok()));
                std::env::set_var(key, value);
            }
            Self {
                _lock: lock,
                entries: previous,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, previous) in &self.entries {
                if let Some(value) = previous {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }

    #[test]
    fn test_create_jwt() {
        let _guard = EnvGuard::set(&[("RMC_JWT_SECRET", "test-secret")]);
        let token = create_jwt("test_user").unwrap();
        assert!(!token.is_empty());
    }

    #[test]
    fn test_verify_jwt() {
        let _guard = EnvGuard::set(&[("RMC_JWT_SECRET", "test-secret")]);
        let token = create_jwt("test_user").unwrap();
        let claims = verify_jwt(&token).unwrap();
        assert_eq!(claims.sub, "test_user");
    }

    #[test]
    fn test_validate_admin_login_with_env_override() {
        let _guard = EnvGuard::set(&[
            ("RMC_ADMIN_USERNAME", "operator"),
            ("RMC_ADMIN_PASSWORD", "strong-password"),
        ]);
        assert!(validate_admin_login("operator", "strong-password"));
        assert!(!validate_admin_login("operator", "wrong"));
    }
}
