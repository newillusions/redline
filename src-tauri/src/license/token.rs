//! Offline Ed25519 verification of the redline entitlement token minted by
//! emittiv-staff's `license.ts` (S2a). Mirrors `verifyToken` there exactly:
//! the signature covers the raw UTF-8 bytes of the base64url PAYLOAD SEGMENT,
//! not the re-serialized JSON, so this side never needs to reproduce field
//! order/whitespace to check a signature.
//!
//! Pure, no IO. The device-binding check (does payload.device_fingerprint
//! match THIS install) lives one layer up in `gate.rs`, since it needs the
//! locally persisted device id, not just the token bytes.

use base64::{engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Mirrors `LicensePayload` in emittiv-staff/src/lib/server/license.ts exactly -
/// field names/shape are the cross-service contract, do not rename.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicensePayload {
    pub staff_id: String,
    pub app: String,
    pub device_fingerprint: String,
    pub issued_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyFailureReason {
    Malformed,
    BadSignature,
    Expired,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VerifyResult {
    Valid(LicensePayload),
    Invalid {
        reason: VerifyFailureReason,
        payload: Option<LicensePayload>,
    },
}

fn invalid(reason: VerifyFailureReason) -> VerifyResult {
    VerifyResult::Invalid { reason, payload: None }
}

/// Verify a token's signature and expiry against `public_key`. Offline - no DB
/// access, no clock reads other than the passed-in `now`. Reference behavior
/// matches `verifyToken` in emittiv-staff/src/lib/server/license.ts field for
/// field: malformed on a bad shape, bad_signature on a tamper, expired when
/// `expires_at <= now`.
pub fn verify_token(token: &str, public_key: &VerifyingKey, now: DateTime<Utc>) -> VerifyResult {
    // Reject anything but exactly two non-empty dot-separated segments (the TS
    // reference checks `parts.length !== 2 || !parts[0] || !parts[1]`).
    if token.matches('.').count() != 1 {
        return invalid(VerifyFailureReason::Malformed);
    }
    let Some((payload_segment, signature_segment)) = token.split_once('.') else {
        return invalid(VerifyFailureReason::Malformed);
    };
    if payload_segment.is_empty() || signature_segment.is_empty() {
        return invalid(VerifyFailureReason::Malformed);
    }

    let Ok(sig_bytes) = URL_SAFE_NO_PAD.decode(signature_segment) else {
        return invalid(VerifyFailureReason::Malformed);
    };
    let Ok(sig_array): Result<[u8; 64], _> = sig_bytes.as_slice().try_into() else {
        return invalid(VerifyFailureReason::Malformed);
    };
    let signature = Signature::from_bytes(&sig_array);

    // Ed25519 signs the raw UTF-8 bytes of the base64url PAYLOAD SEGMENT
    // itself (not the decoded JSON) - matches license.ts's
    // cryptoSign(null, Buffer.from(payloadSegment, 'utf8'), privateKey).
    if public_key
        .verify_strict(payload_segment.as_bytes(), &signature)
        .is_err()
    {
        return invalid(VerifyFailureReason::BadSignature);
    }

    let Ok(payload_json) = URL_SAFE_NO_PAD.decode(payload_segment) else {
        return invalid(VerifyFailureReason::Malformed);
    };
    let Ok(payload) = serde_json::from_slice::<LicensePayload>(&payload_json) else {
        return invalid(VerifyFailureReason::Malformed);
    };

    let Ok(expires_at) = DateTime::parse_from_rfc3339(&payload.expires_at) else {
        return VerifyResult::Invalid {
            reason: VerifyFailureReason::Malformed,
            payload: Some(payload),
        };
    };

    if expires_at.with_timezone(&Utc) <= now {
        return VerifyResult::Invalid {
            reason: VerifyFailureReason::Expired,
            payload: Some(payload),
        };
    }

    VerifyResult::Valid(payload)
}

/// Parse the baked SPKI-DER PEM into a raw Ed25519 `VerifyingKey`.
///
/// Ed25519 SPKI DER is a fixed 12-byte prefix (`302a300506032b6570032100`,
/// RFC 8410) followed by the 32 raw key bytes, so no general ASN.1 parser is
/// needed - just base64-decode the PEM body (standard alphabet, WITH padding;
/// PEM is never base64url) and take the last 32 bytes.
///
/// Panics on a malformed constant. This only ever runs against the compile-
/// time-baked `LICENSE_PUBLIC_KEY_PEM`, so a panic here means the constant
/// itself is broken - a build-time bug, not a runtime condition a user can hit.
pub fn parse_public_key_pem(pem: &str) -> VerifyingKey {
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    let der = STANDARD
        .decode(body)
        .expect("LICENSE_PUBLIC_KEY_PEM: invalid base64 (build-time constant is broken)");
    let key_bytes: [u8; 32] = der
        .get(der.len().saturating_sub(32)..)
        .expect("LICENSE_PUBLIC_KEY_PEM: too short to contain a key")
        .try_into()
        .expect("LICENSE_PUBLIC_KEY_PEM: key is not 32 bytes");
    VerifyingKey::from_bytes(&key_bytes).expect("LICENSE_PUBLIC_KEY_PEM: not a valid Ed25519 point")
}

/// Mint a token the same way emittiv-staff's `mintToken` does, for tests only.
/// Keeps token.rs/gate.rs/service.rs tests self-contained without a live
/// license service or a copy of the TypeScript signer.
#[cfg(test)]
pub(crate) fn mint_token_for_tests(
    signing_key: &ed25519_dalek::SigningKey,
    payload: &LicensePayload,
) -> String {
    use ed25519_dalek::Signer;
    let payload_json = serde_json::to_vec(payload).expect("payload serializes");
    let payload_segment = URL_SAFE_NO_PAD.encode(payload_json);
    let signature = signing_key.sign(payload_segment.as_bytes());
    let signature_segment = URL_SAFE_NO_PAD.encode(signature.to_bytes());
    format!("{payload_segment}.{signature_segment}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use ed25519_dalek::SigningKey;

    fn test_keypair() -> (SigningKey, VerifyingKey) {
        // Fixed 32-byte seed - deterministic keys, no RNG dependency in tests.
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let verifying_key = VerifyingKey::from(&signing_key);
        (signing_key, verifying_key)
    }

    fn sample_payload(expires_at: DateTime<Utc>) -> LicensePayload {
        LicensePayload {
            staff_id: "staff:abc123".to_string(),
            app: "redline".to_string(),
            device_fingerprint: "device-xyz".to_string(),
            issued_at: Utc::now().to_rfc3339(),
            expires_at: expires_at.to_rfc3339(),
        }
    }

    #[test]
    fn verify_valid_token() {
        let (sk, vk) = test_keypair();
        let now = Utc::now();
        let payload = sample_payload(now + Duration::days(14));
        let token = mint_token_for_tests(&sk, &payload);
        match verify_token(&token, &vk, now) {
            VerifyResult::Valid(p) => assert_eq!(p, payload),
            other => panic!("expected valid, got {other:?}"),
        }
    }

    #[test]
    fn verify_expired_token() {
        let (sk, vk) = test_keypair();
        let now = Utc::now();
        let payload = sample_payload(now - Duration::days(1));
        let token = mint_token_for_tests(&sk, &payload);
        match verify_token(&token, &vk, now) {
            VerifyResult::Invalid {
                reason: VerifyFailureReason::Expired,
                payload: Some(p),
            } => assert_eq!(p, payload),
            other => panic!("expected expired, got {other:?}"),
        }
    }

    #[test]
    fn verify_tampered_payload_rejected() {
        let (sk, vk) = test_keypair();
        let now = Utc::now();
        let payload = sample_payload(now + Duration::days(14));
        let token = mint_token_for_tests(&sk, &payload);
        let (payload_segment, signature_segment) = token.split_once('.').unwrap();

        // Flip the last character of the payload segment without re-signing -
        // the stored signature no longer matches the (now different) bytes.
        let mut chars: Vec<char> = payload_segment.chars().collect();
        let last = chars.len() - 1;
        chars[last] = if chars[last] == 'A' { 'B' } else { 'A' };
        let tampered_segment: String = chars.into_iter().collect();
        let tampered = format!("{tampered_segment}.{signature_segment}");

        match verify_token(&tampered, &vk, now) {
            VerifyResult::Invalid {
                reason: VerifyFailureReason::BadSignature,
                ..
            } => {}
            other => panic!("expected bad_signature, got {other:?}"),
        }
    }

    #[test]
    fn verify_malformed_token_rejected() {
        let (_, vk) = test_keypair();
        match verify_token("not-a-token", &vk, Utc::now()) {
            VerifyResult::Invalid {
                reason: VerifyFailureReason::Malformed,
                ..
            } => {}
            other => panic!("expected malformed, got {other:?}"),
        }
        match verify_token("a.b.c", &vk, Utc::now()) {
            VerifyResult::Invalid {
                reason: VerifyFailureReason::Malformed,
                ..
            } => {}
            other => panic!("expected malformed for 3-segment token, got {other:?}"),
        }
    }

    #[test]
    fn parses_baked_public_key_pem() {
        // Smoke test: the actual constant parses to a valid Ed25519 point.
        let _ = parse_public_key_pem(crate::license::LICENSE_PUBLIC_KEY_PEM);
    }
}
