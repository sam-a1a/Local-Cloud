use anyhow::Result;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;

/// Represents the permanent cryptographic identity of a device on the mesh
pub struct DeviceIdentity {
    pub device_id: String,       // Hex-encoded Ed25519 public key
    pub signing_key: SigningKey, // Ed25519 private key (keep secret!)
    pub cert_pem: String,        // TLS self-signed certificate
    pub key_pem: String,         // TLS private key
}

impl DeviceIdentity {
    /// Generates a new random identity and self-signed TLS cert on first run
    pub fn generate() -> Result<Self> {
        // 1. Generate Ed25519 Keypair
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key: VerifyingKey = signing_key.verifying_key();
        
        // Encode public key to hex string to use as Device ID
        let device_id = hex::encode(verifying_key.to_bytes());

        // 2. Generate self-signed TLS certificate for mTLS
        // We use the device_id as the Common Name (CN) for the cert
        let cert_key = rcgen::generate_simple_self_signed(vec![device_id.clone()])?;
        let cert_pem = cert_key.cert.pem();
        
        let key_pem = cert_key.signing_key.serialize_pem();

        Ok(Self {
            device_id,
            signing_key,
            cert_pem,
            key_pem,
        })
    }
}