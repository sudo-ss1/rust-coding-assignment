use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};

/// A valid Argon2id PHC string for the password "not-a-real-password", used to
/// burn the same CPU time when the email is unknown as when it is known.
/// Without this, response latency discloses which emails are registered.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$\
                          NfEnUOuUlLEjqAX0X0Zt6W6mMPMYHnBb05NhkF8Vv1Q";

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    Ok(Argon2::default()
        .hash_password(password.as_bytes())?
        .to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

/// Run a verification against a fixed hash and discard the result, so that the
/// unknown-email path costs the same as the wrong-password path.
pub fn verify_dummy() {
    let _ = Argon2::default().verify_password(b"not-a-real-password", DUMMY_HASH);
}
