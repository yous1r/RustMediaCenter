use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
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

    // Note: 硬编码的密钥仅做示例，生产应走 config
    encode(&Header::default(), &claims, &EncodingKey::from_secret(b"secret"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_jwt() {
        let token = create_jwt("test_user").unwrap();
        assert!(!token.is_empty());
    }
}
