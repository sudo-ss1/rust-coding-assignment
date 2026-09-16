use crate::domain::Role;
use chrono::Utc;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub role: Role,
    pub jti: Uuid,
    pub iat: i64,
    pub exp: i64,
}

impl Claims {
    pub fn new(user_id: Uuid, role: Role, ttl_seconds: i64) -> Self {
        let now = Utc::now().timestamp();
        Self {
            sub: user_id,
            role,
            jti: Uuid::new_v4(),
            iat: now,
            exp: now + ttl_seconds,
        }
    }

    /// Seconds until this token expires, floored at zero. Used as the TTL of the
    /// denylist entry so it disappears exactly when the token would have anyway.
    pub fn seconds_remaining(&self) -> u64 {
        (self.exp - Utc::now().timestamp()).max(0) as u64
    }
}

pub fn encode_token(secret: &str, claims: &Claims) -> Result<String, jsonwebtoken::errors::Error> {
    encode(
        &Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

pub fn decode_token(secret: &str, token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )?;
    Ok(data.claims)
}
