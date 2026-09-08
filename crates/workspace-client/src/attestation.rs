//! Short-lived, single-request proofs from configured service executors.
//!
//! Identity still authenticates the user on every request. These proofs authenticate the host
//! making a downward call and bind its method, path, exact JSON bytes and transient session.
//! Workspace consumes each verified nonce durably before admitting an effect. Proofs do not
//! substitute for the current session, grants, attempt lifecycle, or operation idempotency.

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Authentication proof header. Its value is never a caller-set actor coordinate.
pub const PROOF_HEADER: &str = "workspace-request-proof";
/// Upper bound on a previously issued proof after an executor stops issuing requests.
pub const PROOF_TTL_SECONDS: u64 = 10;
const AUDIENCE: &str = "urn:b10x:workspace:request:v1";

/// Separately configured trust roles; a coordinator cannot admit a coding attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostRole {
    Executor,
    Coordinator,
}

impl HostRole {
    fn issuer(self) -> &'static str {
        match self {
            Self::Executor => "urn:b10x:workspace:executor",
            Self::Coordinator => "urn:b10x:workspace:coordinator",
        }
    }
}

/// Deliberately body-free authentication refusal.
#[derive(Debug, thiserror::Error)]
#[error("Workspace service request proof is invalid or unavailable")]
pub struct ProofError;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    iss: String,
    aud: String,
    iat: u64,
    nbf: u64,
    exp: u64,
    jti: String,
    session_sha256: String,
    request_sha256: String,
}

/// Private signing material remains inside the host authentication adapter.
pub struct RequestSigner {
    role: HostRole,
    key: EncodingKey,
}

impl RequestSigner {
    /// Parse a configured Ed25519 private key. Callers must zeroize their source buffer.
    pub fn from_pem(role: HostRole, pem: &[u8]) -> Result<Self, ProofError> {
        Ok(Self {
            role,
            key: EncodingKey::from_ed_pem(pem).map_err(|_| ProofError)?,
        })
    }

    /// Attest one request only after checking the host's current task or orchestration authority.
    pub fn sign(
        &self,
        session: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<Zeroizing<String>, ProofError> {
        let issued = now()?;
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(|_| ProofError)?;
        let claims = Claims {
            iss: self.role.issuer().into(),
            aud: AUDIENCE.into(),
            iat: issued,
            nbf: issued,
            exp: issued.checked_add(PROOF_TTL_SECONDS).ok_or(ProofError)?,
            jti: hex::encode(random),
            session_sha256: digest(session.as_bytes()),
            request_sha256: request_digest(method, path, body),
        };
        encode(&Header::new(Algorithm::EdDSA), &claims, &self.key)
            .map(Zeroizing::new)
            .map_err(|_| ProofError)
    }
}

/// Verification is tied to an explicit role and its configured public key, never token-selected keys.
#[derive(Clone)]
pub struct RequestVerifier {
    role: HostRole,
    key: DecodingKey,
}

/// A verified, body-free receipt for Workspace's durable replay guard.
pub struct VerifiedRequest {
    nonce: String,
    issuer: String,
    expires_at: u64,
    session_sha256: String,
    request_sha256: String,
}

impl VerifiedRequest {
    pub fn session_sha256(&self) -> &str {
        &self.session_sha256
    }
    pub fn request_sha256(&self) -> &str {
        &self.request_sha256
    }
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

impl RequestVerifier {
    pub fn from_pem(role: HostRole, pem: &[u8]) -> Result<Self, ProofError> {
        Ok(Self {
            role,
            key: DecodingKey::from_ed_pem(pem).map_err(|_| ProofError)?,
        })
    }

    /// Verify signature, role, audience, bounded lifetime, session and exact request bindings.
    /// The result still needs atomic replay consumption and current domain authorization.
    pub fn verify(
        &self,
        proof: &str,
        session: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<VerifiedRequest, ProofError> {
        if proof.len() > 4096 {
            return Err(ProofError);
        }
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.leeway = 0;
        validation.validate_nbf = true;
        validation.set_audience(&[AUDIENCE]);
        validation.set_issuer(&[self.role.issuer()]);
        validation.set_required_spec_claims(&["exp", "nbf", "iss", "aud"]);
        let claims = decode::<Claims>(proof, &self.key, &validation)
            .map_err(|_| ProofError)?
            .claims;
        let current = now()?;
        if claims.iat > current
            || claims.nbf != claims.iat
            || claims.exp <= current
            || claims.exp.checked_sub(claims.iat) != Some(PROOF_TTL_SECONDS)
            || claims.jti.len() != 64
            || !claims.jti.bytes().all(|b| b.is_ascii_hexdigit())
            || claims.session_sha256 != digest(session.as_bytes())
            || claims.request_sha256 != request_digest(method, path, body)
        {
            return Err(ProofError);
        }
        Ok(VerifiedRequest {
            nonce: claims.jti,
            issuer: claims.iss,
            expires_at: claims.exp,
            session_sha256: claims.session_sha256,
            request_sha256: claims.request_sha256,
        })
    }
}

fn now() -> Result<u64, ProofError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(|_| ProofError)
}

fn digest(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn request_digest(method: &str, path: &str, body: &[u8]) -> String {
    let mut digest = Sha256::new();
    for part in [method.as_bytes(), path.as_bytes(), body] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    hex::encode(digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn pair(role: HostRole) -> (RequestSigner, RequestVerifier) {
        let der = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .expect("ephemeral test key");
        let pair = Ed25519KeyPair::from_pkcs8(der.as_ref()).expect("test key pair");
        let private = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
            base64::engine::general_purpose::STANDARD.encode(der.as_ref())
        );
        // RFC 8410 Ed25519 SubjectPublicKeyInfo prefix followed by the public key.
        let mut public = vec![
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0,
        ];
        public.extend_from_slice(pair.public_key().as_ref());
        let public = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
            base64::engine::general_purpose::STANDARD.encode(public)
        );
        (
            RequestSigner::from_pem(role, private.as_bytes()).expect("private PEM"),
            RequestVerifier::from_pem(role, public.as_bytes()).expect("public PEM"),
        )
    }

    #[test]
    fn proofs_bind_every_request_coordinate_and_trust_role() {
        let (signer, verifier) = pair(HostRole::Executor);
        let proof = signer
            .sign("Bearer session", "POST", "/attempts/one", b"{}")
            .unwrap();
        let receipt = verifier
            .verify(&proof, "Bearer session", "POST", "/attempts/one", b"{}")
            .expect("exact request admitted for durable consumption");
        assert_eq!(receipt.issuer(), HostRole::Executor.issuer());
        for (session, method, path, body) in [
            ("Bearer another", "POST", "/attempts/one", b"{}".as_slice()),
            ("Bearer session", "GET", "/attempts/one", b"{}".as_slice()),
            ("Bearer session", "POST", "/attempts/two", b"{}".as_slice()),
            ("Bearer session", "POST", "/attempts/one", b"{ }".as_slice()),
        ] {
            assert!(
                verifier
                    .verify(&proof, session, method, path, body)
                    .is_err(),
                "changed request coordinate must refuse proof"
            );
        }
        let (_, other_key) = pair(HostRole::Executor);
        assert!(
            other_key
                .verify(&proof, "Bearer session", "POST", "/attempts/one", b"{}")
                .is_err(),
            "unconfigured signing key must refuse"
        );
        let wrong_role = RequestVerifier {
            role: HostRole::Coordinator,
            key: verifier.key.clone(),
        };
        assert!(
            wrong_role
                .verify(&proof, "Bearer session", "POST", "/attempts/one", b"{}")
                .is_err(),
            "coordinator trust must not accept executor issuer even with the same public key"
        );
        let second = signer
            .sign("Bearer session", "POST", "/attempts/one", b"{}")
            .unwrap();
        let second = verifier
            .verify(&second, "Bearer session", "POST", "/attempts/one", b"{}")
            .unwrap();
        assert_ne!(
            receipt.nonce(),
            second.nonce(),
            "legitimate retry needs a fresh nonce"
        );
    }

    #[test]
    fn signed_but_expired_future_or_overlong_claims_are_refused() {
        let (signer, verifier) = pair(HostRole::Executor);
        let current = now().unwrap();
        for (issued, expires, audience) in [
            (current - 20, current - 10, AUDIENCE),
            (current + 60, current + 70, AUDIENCE),
            (current, current + 60, AUDIENCE),
            (current, current + PROOF_TTL_SECONDS, "another-service"),
        ] {
            let claims = Claims {
                iss: HostRole::Executor.issuer().into(),
                aud: audience.into(),
                iat: issued,
                nbf: issued,
                exp: expires,
                jti: "a".repeat(64),
                session_sha256: digest(b"session"),
                request_sha256: request_digest("POST", "/", b"{}"),
            };
            let proof = encode(&Header::new(Algorithm::EdDSA), &claims, &signer.key).unwrap();
            assert!(
                verifier
                    .verify(&proof, "session", "POST", "/", b"{}")
                    .is_err(),
                "signature alone cannot admit a claim outside audience or lifetime bounds"
            );
        }
    }
}
