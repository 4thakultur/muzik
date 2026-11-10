//! Time-based one-time passwords (RFC 6238: HMAC-SHA1, 6 digits, 30 s) and
//! single-use recovery codes, for two-factor sign-in with any authenticator app.

use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, Rng, RngCore};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const ISSUER: &str = "Muzik";
pub const STEP_SECS: u64 = 30;
pub const DIGITS: u32 = 6;
/// Accepted clock drift, in steps on each side (±30 s).
pub const SKEW: i64 = 1;
pub const RECOVERY_CODE_COUNT: usize = 10;
/// Pending (unconfirmed) enrollments expire after this long.
pub const PENDING_TTL_SECS: i64 = 15 * 60;

/// A new random 160-bit secret, base32-encoded (the format authenticator apps expect).
pub fn new_secret() -> String {
    let mut bytes = [0u8; 20];
    OsRng.fill_bytes(&mut bytes);
    BASE32_NOPAD.encode(&bytes)
}

/// `otpauth://` URI for QR codes / manual entry.
pub fn otpauth_uri(secret: &str, username: &str) -> String {
    // Usernames are restricted to [A-Za-z0-9_.-], all URI-safe.
    format!(
        "otpauth://totp/{ISSUER}:{username}?secret={secret}&issuer={ISSUER}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}"
    )
}

/// The URI rendered as a standalone SVG QR code.
pub fn qr_svg(uri: &str) -> Option<String> {
    use qrcode::{render::svg, EcLevel, QrCode};
    let code = QrCode::with_error_correction_level(uri.as_bytes(), EcLevel::M).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(200, 200)
            .quiet_zone(true)
            .dark_color(svg::Color("#000000"))
            .light_color(svg::Color("#ffffff"))
            .build(),
    )
}

pub fn current_step(unix_secs: i64) -> i64 {
    unix_secs.max(0) / STEP_SECS as i64
}

/// The numeric code for one time step.
fn code_at(secret: &[u8], step: i64) -> u32 {
    // HMAC accepts keys of any length; this cannot fail.
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(secret) else {
        return u32::MAX;
    };
    mac.update(&(step as u64).to_be_bytes());
    let h = mac.finalize().into_bytes();
    let offset = (h[h.len() - 1] & 0x0f) as usize;
    let bin = (u32::from(h[offset] & 0x7f) << 24)
        | (u32::from(h[offset + 1]) << 16)
        | (u32::from(h[offset + 2]) << 8)
        | u32::from(h[offset + 3]);
    bin % 10u32.pow(DIGITS)
}

fn decode_secret(secret: &str) -> Option<Vec<u8>> {
    BASE32_NOPAD
        .decode(secret.trim_end_matches('=').as_bytes())
        .ok()
}

/// The code an authenticator app shows at `unix_secs` (for tests and tooling).
pub fn generate(secret: &str, unix_secs: i64) -> Option<String> {
    let key = decode_secret(secret)?;
    Some(format!(
        "{:0width$}",
        code_at(&key, current_step(unix_secs)),
        width = DIGITS as usize
    ))
}

/// Keeps only the digits of user input (`123 456` and `123-456` are accepted).
pub fn normalize_code(input: &str) -> Option<String> {
    let digits: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    (digits.len() == DIGITS as usize && digits.bytes().all(|b| b.is_ascii_digit()))
        .then_some(digits)
}

/// Checks `code` against the steps around `now`, skipping steps at or before
/// `last_step` (a code is accepted only once). Returns the matching step.
/// Every candidate step is compared in constant time.
pub fn verify(secret: &str, code: &str, now_unix: i64, last_step: Option<i64>) -> Option<i64> {
    let key = decode_secret(secret)?;
    let code = normalize_code(code)?;
    let now = current_step(now_unix);
    let mut matched = None;
    for step in (now - SKEW)..=(now + SKEW) {
        let expected = format!("{:0width$}", code_at(&key, step), width = DIGITS as usize);
        let ok: bool = expected.as_bytes().ct_eq(code.as_bytes()).into();
        if ok && last_step.is_none_or(|l| step > l) && matched.is_none() {
            matched = Some(step);
        }
    }
    matched
}

// ---------- Recovery codes ----------

const RECOVERY_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Ten random codes formatted `XXXXX-XXXXX` (50 bits of entropy each).
pub fn new_recovery_codes() -> Vec<String> {
    let mut rng = OsRng;
    (0..RECOVERY_CODE_COUNT)
        .map(|_| {
            let raw: String = (0..10)
                .map(|_| RECOVERY_ALPHABET[rng.gen_range(0..RECOVERY_ALPHABET.len())] as char)
                .collect();
            format!("{}-{}", &raw[..5], &raw[5..])
        })
        .collect()
}

/// Canonical form of a typed recovery code: uppercase, separators removed.
pub fn normalize_recovery_code(input: &str) -> Option<String> {
    let s: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (s.len() == 10 && s.bytes().all(|b| RECOVERY_ALPHABET.contains(&b))).then_some(s)
}

/// Recovery codes are high-entropy random strings, so a plain SHA-256 is
/// enough to store them (no password-style stretching needed).
pub fn recovery_hash(normalized: &str) -> String {
    hex::encode(Sha256::digest(normalized.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 appendix B test secret ("12345678901234567890"), SHA-1.
    const RFC_SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn rfc6238_vectors() {
        let key = decode_secret(RFC_SECRET).unwrap();
        // 8-digit vectors truncated to 6 digits = value mod 10^6.
        for (t, expected) in [
            (59, 94_287_082u32),
            (1_111_111_109, 7_081_804),
            (1_111_111_111, 14_050_471),
            (1_234_567_890, 89_005_924),
            (2_000_000_000, 69_279_037),
        ] {
            assert_eq!(
                code_at(&key, current_step(t)),
                expected % 1_000_000,
                "t={t}"
            );
        }
    }

    #[test]
    fn verify_window_and_replay() {
        let t = 1_234_567_890;
        let key = decode_secret(RFC_SECRET).unwrap();
        let code = format!("{:06}", code_at(&key, current_step(t)));
        let step = verify(RFC_SECRET, &code, t, None).unwrap();
        assert_eq!(step, current_step(t));
        // Still valid 30 s later (clock skew)...
        assert!(verify(RFC_SECRET, &code, t + 30, None).is_some());
        // ...but not 90 s later.
        assert!(verify(RFC_SECRET, &code, t + 90, None).is_none());
        // Replay of an already used step is refused.
        assert!(verify(RFC_SECRET, &code, t, Some(step)).is_none());
        // Formatting is tolerated, garbage is not.
        let spaced = format!("{} {}", &code[..3], &code[3..]);
        assert!(verify(RFC_SECRET, &spaced, t, None).is_some());
        assert!(verify(RFC_SECRET, "12345", t, None).is_none());
        assert!(verify(RFC_SECRET, "abcdef", t, None).is_none());
        assert!(verify("not base32!", &code, t, None).is_none());
    }

    #[test]
    fn secrets_and_uris() {
        let s = new_secret();
        assert_eq!(s.len(), 32);
        assert_eq!(decode_secret(&s).unwrap().len(), 20);
        let uri = otpauth_uri(&s, "alice");
        assert!(uri.starts_with("otpauth://totp/Muzik:alice?secret="));
        assert!(qr_svg(&uri).unwrap().starts_with("<?xml"));
    }

    #[test]
    fn recovery_codes() {
        let codes = new_recovery_codes();
        assert_eq!(codes.len(), RECOVERY_CODE_COUNT);
        let c = &codes[0];
        assert_eq!(c.len(), 11);
        let n = normalize_recovery_code(&c.to_lowercase()).unwrap();
        assert_eq!(n, c.replace('-', ""));
        assert!(normalize_recovery_code("short").is_none());
        assert!(normalize_recovery_code("0000000000").is_none()); // 0 is not in the alphabet
    }
}
