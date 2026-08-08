// engine/src/crypto.rs
use anyhow::Result;
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use rand::rngs::OsRng;
use rand::TryRngCore;

#[derive(Serialize, Deserialize)]
struct IdentityFile {
    signing_key_hex: String,
    cert_pem: String,
    key_pem: String,
    /// Absent in identities written before device naming existed.
    #[serde(default)]
    device_name: String,
}

/// Human-readable platform label, shown beside the device name when picking
/// devices to pair with.
pub fn platform_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        "android" => "Android",
        "ios" => "iOS",
        other => other,
    }
}

/// Best-effort name for this device, e.g. "Sam's MacBook Pro". Users can
/// override it, since some platforms only report something like "localhost".
fn default_device_name() -> String {
    match whoami::devicename() {
        Ok(name) if !name.trim().is_empty() => name,
        _ => format!("Unnamed {}", platform_name()),
    }
}

/// Represents the permanent cryptographic identity of a device on the mesh
pub struct DeviceIdentity {
    pub device_id: String,       // Hex-encoded Ed25519 public key
    pub signing_key: SigningKey, // Ed25519 private key (keep secret!)
    pub cert_pem: String,        // TLS self-signed certificate
    pub key_pem: String,         // TLS private key
    pub device_name: String,     // Shown to peers during pairing
    base_dir: PathBuf,           // Where identity.json lives, for renames
}

impl DeviceIdentity {
    /// Loads an identity from disk if it exists, otherwise generates a new one and saves it.
    pub fn load_or_generate(base_dir: &str) -> Result<Self> {
        let id_path = Path::new(base_dir).join("identity.json");

        if id_path.exists() {
            let data = std::fs::read_to_string(&id_path)?;
            let mut file: IdentityFile = serde_json::from_str(&data)?;

            let key_bytes = hex::decode(&file.signing_key_hex)?;
            let arr: [u8; 32] = key_bytes.as_slice().try_into().map_err(|_| anyhow::anyhow!("Invalid key length"))?;
            let signing_key = SigningKey::from_bytes(&arr);
            let verifying_key: VerifyingKey = signing_key.verifying_key();
            let device_id = hex::encode(verifying_key.to_bytes());

            // Backfill the name for identities created before this field existed.
            if file.device_name.trim().is_empty() {
                file.device_name = default_device_name();
                std::fs::write(&id_path, serde_json::to_string_pretty(&file)?)?;
            }

            Ok(Self {
                device_id,
                signing_key,
                cert_pem: file.cert_pem,
                key_pem: file.key_pem,
                device_name: file.device_name,
                base_dir: PathBuf::from(base_dir),
            })
        } else {
            let identity = Self::generate(base_dir)?;
            let file = IdentityFile {
                signing_key_hex: hex::encode(identity.signing_key.to_bytes()),
                cert_pem: identity.cert_pem.clone(),
                key_pem: identity.key_pem.clone(),
                device_name: identity.device_name.clone(),
            };
            std::fs::write(&id_path, serde_json::to_string_pretty(&file)?)?;
            Ok(identity)
        }
    }

    /// Generates a new random identity and self-signed TLS cert
    pub fn generate(base_dir: &str) -> Result<Self> {
        // The key material is drawn here rather than handed to
        // `SigningKey::generate`, which wants an RNG implementing *its* version
        // of rand_core. ed25519-dalek and rand advance that trait on their own
        // schedules, and coupling this to both at once means an upgrade to
        // either is blocked until they agree. Thirty-two bytes from the
        // operating system is what a signing key is; nothing is given up.
        //
        // Reading it is fallible, and says so: rand_core made OS randomness a
        // `TryRngCore` precisely because it can fail, and a device identity is
        // the last thing that should be built on a silent fallback.
        let mut secret = [0u8; 32];
        OsRng.try_fill_bytes(&mut secret).map_err(|e| {
            anyhow::anyhow!("Could not read randomness from the operating system: {}", e)
        })?;
        let signing_key = SigningKey::from_bytes(&secret);
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
            device_name: default_device_name(),
            base_dir: PathBuf::from(base_dir),
        })
    }

    /// Renames this device. Peers see the new name the next time they resolve
    /// it over mDNS; already-paired devices update it on their next contact.
    pub fn set_device_name(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("Device name cannot be empty");
        }

        let id_path = self.base_dir.join("identity.json");
        let data = std::fs::read_to_string(&id_path)?;
        let mut file: IdentityFile = serde_json::from_str(&data)?;
        file.device_name = name.to_string();
        std::fs::write(&id_path, serde_json::to_string_pretty(&file)?)?;

        self.device_name = name.to_string();
        Ok(())
    }
}
