use std::env;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing environment variable: {0}")]
    Missing(&'static str),
    #[error("invalid value for {0}: {1}")]
    Invalid(&'static str, String),
    #[error("JWT_SECRET must be at least 32 bytes, got {0}")]
    WeakSecret(usize),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub redis_url: String,
    pub jwt_secret: String,
    pub jwt_ttl_seconds: i64,
    pub twofa_ttl_seconds: i64,
    pub twofa_max_attempts: i32,
    pub cache_ttl_seconds: u64,
    pub app_env: String,
    pub bind_addr: String,
    pub seed_admin_password: String,
    pub seed_bond_password: String,
}

fn required(key: &'static str) -> Result<String, ConfigError> {
    env::var(key).map_err(|_| ConfigError::Missing(key))
}

fn parsed<T: std::str::FromStr>(key: &'static str, default: T) -> Result<T, ConfigError> {
    match env::var(key) {
        Err(_) => Ok(default),
        Ok(raw) => raw.parse::<T>().map_err(|_| ConfigError::Invalid(key, raw)),
    }
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let jwt_secret = required("JWT_SECRET")?;
        if jwt_secret.len() < 32 {
            return Err(ConfigError::WeakSecret(jwt_secret.len()));
        }
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            redis_url: required("REDIS_URL")?,
            jwt_secret,
            jwt_ttl_seconds: parsed("JWT_TTL_SECONDS", 900)?,
            twofa_ttl_seconds: parsed("TWOFA_TTL_SECONDS", 300)?,
            twofa_max_attempts: parsed("TWOFA_MAX_ATTEMPTS", 5)?,
            cache_ttl_seconds: parsed("CACHE_TTL_SECONDS", 60)?,
            app_env: env::var("APP_ENV").unwrap_or_else(|_| "production".into()),
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into()),
            seed_admin_password: env::var("SEED_ADMIN_PASSWORD")
                .unwrap_or_else(|_| "admin123".into()),
            seed_bond_password: env::var("SEED_BOND_PASSWORD")
                .unwrap_or_else(|_| "bond007".into()),
        })
    }

    pub fn is_dev(&self) -> bool {
        self.app_env == "dev"
    }
}
