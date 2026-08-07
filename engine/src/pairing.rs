// engine/src/pairing.rs
//
// Pairing turns two devices that merely see each other on the network into two
// devices that trust each other.
//
// The initiator picks devices from the discovery list and displays a random
// 6-digit code. Each target prompts for that code and, on entry, proves
// knowledge of it back to the initiator. A successful proof causes both sides
// to pin each other's TLS certificate permanently.
//
// The code is never sent verbatim. What travels is
//
//     proof = SHA256(code | fingerprint(initiator cert) | fingerprint(target cert))
//
// so a relay that substitutes its own certificate produces a proof the
// initiator will not accept, and an eavesdropper learns nothing reusable.
//
// Limits worth knowing: an active man-in-the-middle who captures a proof can
// brute-force six digits offline in milliseconds. Closing that needs a
// password-authenticated key exchange such as SPAKE2, where the code is never
// committed to at all. The expiry and attempt cap here bound *online* guessing
// only. On a home network this is a deliberate trade; it is not adequate on a
// hostile one.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a displayed code stays valid.
pub const CODE_TTL: Duration = Duration::from_secs(180);

/// Wrong entries tolerated before the whole pairing attempt is abandoned.
pub const MAX_ATTEMPTS: u32 = 5;

/// How long an unanswered incoming request is offered to the user.
const OFFER_TTL: Duration = Duration::from_secs(300);

/// Everything one device needs to know to pin another.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub cert_pem: String,
}

/// An incoming pairing request awaiting a code from the user.
#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct PairingOffer {
    pub device_id: String,
    pub name: String,
    pub platform: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error("No pairing is in progress")]
    NotInProgress,
    #[error("The pairing code has expired")]
    Expired,
    #[error("This device was not selected for pairing")]
    NotSelected,
    #[error("Too many incorrect attempts; pairing cancelled")]
    TooManyAttempts,
    #[error("Incorrect code")]
    BadCode,
    #[error("No pending request from that device")]
    NoSuchOffer,
}

/// Fingerprint of a certificate as exchanged. Both sides hash the identical
/// PEM text, so this needs no DER parsing to agree.
fn fingerprint(cert_pem: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(cert_pem.trim().as_bytes());
    hex::encode(hasher.finalize())
}

/// Binds the code to the exact pair of certificates being exchanged.
pub fn pairing_proof(code: &str, initiator_cert_pem: &str, target_cert_pem: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(code.trim().as_bytes());
    hasher.update(b"|");
    hasher.update(fingerprint(initiator_cert_pem).as_bytes());
    hasher.update(b"|");
    hasher.update(fingerprint(target_cert_pem).as_bytes());
    hex::encode(hasher.finalize())
}

/// Length-independent, early-exit-free comparison.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn generate_code() -> String {
    use rand::Rng;
    format!("{:06}", rand::rng().random_range(0..1_000_000u32))
}

/// The initiator's side: one code, the devices it was shown for, and how many
/// wrong entries have come back.
#[derive(Debug)]
struct Outgoing {
    code: String,
    created_at: Instant,
    pending: HashSet<String>,
    attempts: u32,
}

#[derive(Debug, Default)]
struct Inner {
    outgoing: Option<Outgoing>,
    incoming: HashMap<String, (DeviceInfo, Instant)>,
}

/// Shared, in-memory pairing state. Deliberately not persisted: an interrupted
/// pairing should not survive a restart.
#[derive(Clone, Debug, Default)]
pub struct PairingState {
    inner: Arc<Mutex<Inner>>,
}

impl PairingState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts pairing with the given devices and returns the code to display.
    /// Replaces any pairing already in progress.
    pub fn begin(&self, targets: Vec<String>) -> String {
        let code = generate_code();
        let mut inner = self.inner.lock().unwrap();
        inner.outgoing = Some(Outgoing {
            code: code.clone(),
            created_at: Instant::now(),
            pending: targets.into_iter().collect(),
            attempts: 0,
        });
        code
    }

    /// The code currently on screen, if one is still valid.
    pub fn active_code(&self) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        inner
            .outgoing
            .as_ref()
            .filter(|o| o.created_at.elapsed() < CODE_TTL)
            .map(|o| o.code.clone())
    }

    /// Devices still waiting to enter the current code.
    pub fn awaiting(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        inner
            .outgoing
            .as_ref()
            .filter(|o| o.created_at.elapsed() < CODE_TTL)
            .map(|o| o.pending.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn cancel(&self) {
        self.inner.lock().unwrap().outgoing = None;
    }

    /// Target side: records that a device wants to pair, so the UI can prompt.
    pub fn record_offer(&self, info: DeviceInfo) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .incoming
            .retain(|_, (_, at)| at.elapsed() < OFFER_TTL);
        inner
            .incoming
            .insert(info.device_id.clone(), (info, Instant::now()));
    }

    /// Pairing requests currently awaiting a code from the user.
    pub fn offers(&self) -> Vec<PairingOffer> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .incoming
            .retain(|_, (_, at)| at.elapsed() < OFFER_TTL);
        inner
            .incoming
            .values()
            .map(|(info, _)| PairingOffer {
                device_id: info.device_id.clone(),
                name: info.name.clone(),
                platform: info.platform.clone(),
            })
            .collect()
    }

    /// Looks up an offer without consuming it; the caller removes it only once
    /// pairing has actually succeeded.
    pub fn offer_details(&self, device_id: &str) -> Option<DeviceInfo> {
        let inner = self.inner.lock().unwrap();
        inner
            .incoming
            .get(device_id)
            .filter(|(_, at)| at.elapsed() < OFFER_TTL)
            .map(|(info, _)| info.clone())
    }

    pub fn clear_offer(&self, device_id: &str) {
        self.inner.lock().unwrap().incoming.remove(device_id);
    }

    /// Initiator side: checks a proof returned by a target.
    ///
    /// On success the device is removed from the pending set, and the whole
    /// attempt is cleared once every selected device has paired. On too many
    /// failures the attempt is abandoned, forcing a fresh code.
    pub fn verify(
        &self,
        device_id: &str,
        proof: &str,
        my_cert_pem: &str,
        their_cert_pem: &str,
    ) -> Result<(), PairError> {
        let mut inner = self.inner.lock().unwrap();
        let outgoing = inner.outgoing.as_mut().ok_or(PairError::NotInProgress)?;

        if outgoing.created_at.elapsed() >= CODE_TTL {
            inner.outgoing = None;
            return Err(PairError::Expired);
        }

        if !outgoing.pending.contains(device_id) {
            return Err(PairError::NotSelected);
        }

        let expected = pairing_proof(&outgoing.code, my_cert_pem, their_cert_pem);
        if !constant_time_eq(&expected, proof) {
            outgoing.attempts += 1;
            if outgoing.attempts >= MAX_ATTEMPTS {
                inner.outgoing = None;
                return Err(PairError::TooManyAttempts);
            }
            return Err(PairError::BadCode);
        }

        outgoing.pending.remove(device_id);
        if outgoing.pending.is_empty() {
            inner.outgoing = None;
        }
        Ok(())
    }
}

/// HTTP client for the pairing exchange only.
///
/// It presents no client certificate and accepts whatever the far end offers,
/// because neither side trusts the other yet - that is the entire point of
/// pairing. What makes the exchange safe is the proof binding it to the
/// certificates actually presented, not the transport.
pub fn build_pairing_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(10))
        .build()?)
}

/// Initiator -> target: "I would like to pair, here is who I am."
pub async fn send_pair_request(
    client: &reqwest::Client,
    peer_url: &str,
    me: &DeviceInfo,
) -> anyhow::Result<()> {
    let response = client
        .post(format!("{}/pair_request", peer_url))
        .json(me)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("Pairing request rejected: {}", response.status());
    }
    Ok(())
}

/// Target -> initiator: "here is proof I know the code you are showing."
/// Returns the initiator's details so the target can pin them in turn.
pub async fn send_pair_confirm(
    client: &reqwest::Client,
    peer_url: &str,
    me: &DeviceInfo,
    proof: &str,
) -> anyhow::Result<DeviceInfo> {
    let response = client
        .post(format!("{}/pair_confirm", peer_url))
        .json(&serde_json::json!({ "device": me, "proof": proof }))
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        anyhow::bail!("{}", if detail.is_empty() { status.to_string() } else { detail });
    }

    Ok(response.json().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CERT_A: &str = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----";
    const CERT_B: &str = "-----BEGIN CERTIFICATE-----\nBBBB\n-----END CERTIFICATE-----";
    const CERT_EVIL: &str = "-----BEGIN CERTIFICATE-----\nEVIL\n-----END CERTIFICATE-----";

    #[test]
    fn correct_code_pairs() {
        let state = PairingState::new();
        let code = state.begin(vec!["b".into()]);
        let proof = pairing_proof(&code, CERT_A, CERT_B);
        assert!(state.verify("b", &proof, CERT_A, CERT_B).is_ok());
    }

    #[test]
    fn code_is_six_digits() {
        let state = PairingState::new();
        let code = state.begin(vec!["b".into()]);
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn wrong_code_is_rejected() {
        let state = PairingState::new();
        state.begin(vec!["b".into()]);
        let proof = pairing_proof("000000", CERT_A, CERT_B);
        // A correct guess would be a 1-in-a-million flake, so allow for it.
        if let Err(e) = state.verify("b", &proof, CERT_A, CERT_B) {
            assert!(matches!(e, PairError::BadCode));
        }
    }

    #[test]
    fn substituted_certificate_breaks_the_proof() {
        let state = PairingState::new();
        let code = state.begin(vec!["b".into()]);
        // A relay knows the code but presents its own certificate.
        let proof = pairing_proof(&code, CERT_A, CERT_EVIL);
        assert!(matches!(
            state.verify("b", &proof, CERT_A, CERT_B),
            Err(PairError::BadCode)
        ));
    }

    #[test]
    fn unselected_device_cannot_pair() {
        let state = PairingState::new();
        let code = state.begin(vec!["b".into()]);
        let proof = pairing_proof(&code, CERT_A, CERT_B);
        assert!(matches!(
            state.verify("c", &proof, CERT_A, CERT_B),
            Err(PairError::NotSelected)
        ));
    }

    #[test]
    fn attempts_are_capped() {
        let state = PairingState::new();
        state.begin(vec!["b".into()]);
        let wrong = pairing_proof("not-the-code", CERT_A, CERT_B);

        for _ in 0..MAX_ATTEMPTS - 1 {
            assert!(matches!(
                state.verify("b", &wrong, CERT_A, CERT_B),
                Err(PairError::BadCode)
            ));
        }
        assert!(matches!(
            state.verify("b", &wrong, CERT_A, CERT_B),
            Err(PairError::TooManyAttempts)
        ));
        // The attempt is gone, so even the right code no longer works.
        assert!(matches!(
            state.verify("b", &wrong, CERT_A, CERT_B),
            Err(PairError::NotInProgress)
        ));
    }

    #[test]
    fn multi_device_pairing_clears_only_when_all_are_done() {
        let state = PairingState::new();
        let code = state.begin(vec!["b".into(), "c".into()]);

        let proof_b = pairing_proof(&code, CERT_A, CERT_B);
        assert!(state.verify("b", &proof_b, CERT_A, CERT_B).is_ok());
        assert!(state.active_code().is_some());

        let proof_c = pairing_proof(&code, CERT_A, CERT_EVIL);
        assert!(state.verify("c", &proof_c, CERT_A, CERT_EVIL).is_ok());
        assert!(state.active_code().is_none());
    }

    #[test]
    fn offers_round_trip() {
        let state = PairingState::new();
        state.record_offer(DeviceInfo {
            device_id: "a".into(),
            name: "MacBook".into(),
            platform: "macOS".into(),
            cert_pem: CERT_A.into(),
        });

        assert_eq!(state.offers().len(), 1);
        assert_eq!(state.offer_details("a").unwrap().name, "MacBook");

        state.clear_offer("a");
        assert!(state.offers().is_empty());
    }
}
