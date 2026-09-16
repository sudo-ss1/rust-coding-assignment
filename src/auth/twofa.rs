use rand::Rng;

/// A 6-digit code from the OS random number generator. `rand::rng()` is seeded
/// from the OS entropy source, never from the clock or PID, so codes are not
/// predictable from the time a login was observed.
pub fn generate_code() -> String {
    let n: u32 = rand::rng().random_range(0..1_000_000);
    format!("{n:06}")
}
