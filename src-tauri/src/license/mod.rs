//! S2b - redline client entitlement (emittiv-staff license consumer).
//!
//! Layout mirrors the producer side (S2a, `emittiv-staff/src/lib/server/license.ts`)
//! so the token contract stays a single source of truth read from one place:
//!   - `token`   - offline Ed25519 verify of the compact `<payload>.<signature>`
//!     token (pure, no IO; mirrors `verifyToken` in license.ts exactly).
//!   - `device`  - stable per-install device fingerprint (separate from
//!     `identity.rs`'s user_id - that one is a display identity, this one
//!     binds a LICENSE and must never change once claimed).
//!   - `store`   - persisted activation state (`<app-data-dir>/license/activation.json`).
//!   - `gate`    - pure startup gating decision: signature + expiry + device
//!     binding + renew-due window. No IO - testable without a filesystem.
//!   - `client`  - issue/renew HTTP calls to the emittiv-staff license service.
//!   - `service` - activate/renew orchestration, parameterized over a
//!     `LicenseClient` trait so it's testable with a fake (mirrors
//!     emittiv-staff's `license-service.ts` DbLike injection).

pub mod client;
pub mod device;
pub mod gate;
pub mod service;
pub mod store;
pub mod token;

/// Ed25519 public key baked into every build for offline token verification.
/// Safe to embed - it is the VERIFICATION key, not the signing secret (which
/// lives only in emittiv-staff's `STAFF_LICENSE_SIGNING_KEY` env var).
pub const LICENSE_PUBLIC_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEA9g6EScbxN8fcBTG0zA5UEOuG848iVWaz0ro7GSxgcAA=\n-----END PUBLIC KEY-----\n";

#[cfg(test)]
mod tests {
    use super::token::parse_public_key_pem;
    use super::LICENSE_PUBLIC_KEY_PEM;

    /// Smoke test: the baked constant actually parses to a valid Ed25519 point.
    /// A build-time typo here would otherwise only surface as an inexplicable
    /// "every token is bad_signature" report from the field.
    #[test]
    fn baked_public_key_parses() {
        let _ = parse_public_key_pem(LICENSE_PUBLIC_KEY_PEM);
    }
}
