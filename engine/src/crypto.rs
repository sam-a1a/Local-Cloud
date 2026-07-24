// engine/src/crypto.rs
use anyhow::Result;
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::path::Path;
use rand::rngs::OsRng;

#[derive(Serialize, Deserialize)]
struct IdentityFile {
    signing_key_hex: String,
    cert_pem: String,
    key_pem: String,
}

/// Represents the permanent cryptographic identity of a device on the mesh
pub struct DeviceIdentity {
    pub device_id: String,       // Hex-encoded Ed25519 public key
    pub signing_key: SigningKey, // Ed25519 private key (keep secret!)
    pub cert_pem: String,        // TLS self-signed certificate
    pub key_pem: String,         // TLS private key
}

impl DeviceIdentity {
    /// Loads an identity from disk if it exists, otherwise generates a new one and saves it.
    pub fn load_or_generate(base_dir: &str) -> Result<Self> {
        let id_path = Path::new(base_dir).join("identity.json");

        if id_path.exists() {
            let data = std::fs::read_to_string(&id_path)?;
            let file: IdentityFile = serde_json::from_str(&data)?;

            let key_bytes = hex::decode(&file.signing_key_hex)?;
            let arr: [u8; 32] = key_bytes.as_slice().try_into().map_err(|_| anyhow::anyhow!("Invalid key length"))?;
            let signing_key = SigningKey::from_bytes(&arr);
            let verifying_key: VerifyingKey = signing_key.verifying_key();
            let device_id = hex::encode(verifying_key.to_bytes());

            Ok(Self {
                device_id,
                signing_key,
                cert_pem: file.cert_pem,
                key_pem: file.key_pem,
            })
        } else {
            let identity = Self::generate()?;
            let file = IdentityFile {
                signing_key_hex: hex::encode(identity.signing_key.to_bytes()),
                cert_pem: identity.cert_pem.clone(),
                key_pem: identity.key_pem.clone(),
            };
            std::fs::write(&id_path, serde_json::to_string_pretty(&file)?)?;
            Ok(identity)
        }
    }

    /// Generates a new random identity and self-signed TLS cert
    pub fn generate() -> Result<Self> {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key: VerifyingKey = signing_key.verifying_key();
        let device_id = hex::encode(verifying_key.to_bytes());

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