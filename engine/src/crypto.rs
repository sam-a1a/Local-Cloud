// engine/src/crypto.rs
use anyhow::Result;
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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

/// Names a platform reports when it has no name to give.
///
/// `whoami::devicename()` does not fail on Android - it succeeds, with the
/// literal string "Unknown", so a device that has never been renamed calls
/// itself that on every screen a person picks devices from. Treating these as
/// the absence they are is what makes the fallback below fire.
const UNHELPFUL_NAMES: [&str; 3] = ["unknown", "localhost", "android"];

/// Best-effort name for this device, e.g. "Sam's MacBook Pro". Applications
/// override it with [`DeviceIdentity::set_device_name`], since a platform that
/// knows the model - Android does, through its own APIs - can do far better
/// than this can from inside Rust.
fn default_device_name() -> String {
    usable_reported_name(whoami::devicename().ok())
        .unwrap_or_else(|| format!("Unnamed {}", platform_name()))
}

/// Whether what the platform reported is a name or a stand-in for one.
///
/// Split out from [`default_device_name`] so the judgement can be tested
/// without a platform to ask - the case that matters is Android, and no test
/// here runs there.
fn usable_reported_name(reported: Option<String>) -> Option<String> {
    let name = reported?;
    let trimmed = name.trim();
    if trimmed.is_empty() || UNHELPFUL_NAMES.contains(&trimmed.to_lowercase().as_str()) {
        return None;
    }
    Some(trimmed.to_string())
}

/// Represents the permanent cryptographic identity of a device on the mesh
pub struct DeviceIdentity {
    pub device_id: String,       // Hex-encoded Ed25519 public key
    pub signing_key: SigningKey, // Ed25519 private key (keep secret!)
    pub cert_pem: String,        // TLS self-signed certificate
    pub key_pem: String,         // TLS private key
    /// Shown to peers during pairing.
    ///
    /// The one part of an identity that is not permanent, so it is the one part
    /// behind a lock - and private, so that nothing can read it once and hold a
    /// copy that a later rename cannot reach. Everything else here is fixed for
    /// the life of the device.
    device_name: Arc<Mutex<String>>,
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
                device_name: Arc::new(Mutex::new(file.device_name)),
                base_dir: PathBuf::from(base_dir),
            })
        } else {
            let identity = Self::generate(base_dir)?;
            let file = IdentityFile {
                signing_key_hex: hex::encode(identity.signing_key.to_bytes()),
                cert_pem: identity.cert_pem.clone(),
                key_pem: identity.key_pem.clone(),
                device_name: identity.device_name(),
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
            device_name: Arc::new(Mutex::new(default_device_name())),
            base_dir: PathBuf::from(base_dir),
        })
    }

    /// What this device calls itself right now.
    pub fn device_name(&self) -> String {
        self.device_name.lock().unwrap().clone()
    }

    /// A live view of the name, for the parts of the engine that answer a peer
    /// asking who this is.
    ///
    /// Handed out rather than cloned so that a rename reaches the running TLS
    /// server without restarting it - a peer that pairs a second after the
    /// rename should be told the new name, not the one the server happened to
    /// be started with.
    pub fn device_name_handle(&self) -> Arc<Mutex<String>> {
        Arc::clone(&self.device_name)
    }

    /// Renames this device, on disk and in memory.
    ///
    /// Takes `&self`: an engine is shared across threads by the time anything
    /// can call this, and the name is behind its own lock precisely so that
    /// renaming does not need exclusive access to the identity as a whole.
    ///
    /// The file is written before the field is updated. If the write fails the
    /// name is unchanged rather than changed-but-not-saved, which would come
    /// back as a rename that silently undid itself at the next start.
    pub fn set_device_name(&self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("Device name cannot be empty");
        }

        let id_path = self.base_dir.join("identity.json");
        let data = std::fs::read_to_string(&id_path)?;
        let mut file: IdentityFile = serde_json::from_str(&data)?;
        file.device_name = name.to_string();
        std::fs::write(&id_path, serde_json::to_string_pretty(&file)?)?;

        *self.device_name.lock().unwrap() = name.to_string();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn identity_in(dir: &TempDir) -> DeviceIdentity {
        DeviceIdentity::load_or_generate(&dir.path().to_string_lossy()).expect("identity")
    }

    #[test]
    fn a_rename_survives_a_reload_without_minting_a_new_identity() {
        let dir = TempDir::new().expect("temp dir");
        let identity = identity_in(&dir);

        identity.set_device_name("Sam's Pixel").expect("rename");
        assert_eq!(identity.device_name(), "Sam's Pixel");

        let reloaded = identity_in(&dir);
        assert_eq!(reloaded.device_name(), "Sam's Pixel");

        // The whole point of the name being the mutable part: renaming a device
        // must not make it a different device. A new id here would unpair it
        // from every peer that had pinned the old one.
        assert_eq!(reloaded.device_id, identity.device_id);
        assert_eq!(reloaded.cert_pem, identity.cert_pem);
    }

    #[test]
    fn a_refused_rename_leaves_the_old_name_in_place() {
        let dir = TempDir::new().expect("temp dir");
        let identity = identity_in(&dir);
        let before = identity.device_name();

        assert!(identity.set_device_name("   ").is_err());
        assert_eq!(identity.device_name(), before);

        // And nothing was written either, so a reload agrees.
        assert_eq!(identity_in(&dir).device_name(), before);
    }

    #[test]
    fn a_name_is_stored_trimmed() {
        let dir = TempDir::new().expect("temp dir");
        let identity = identity_in(&dir);

        identity.set_device_name("  Studio Mac  ").expect("rename");
        assert_eq!(identity.device_name(), "Studio Mac");
    }

    #[test]
    fn a_platform_reporting_no_real_name_is_treated_as_reporting_none() {
        // The Android case, which is why any of this exists: whoami does not
        // fail there, it succeeds with "Unknown".
        assert_eq!(usable_reported_name(Some("Unknown".into())), None);
        assert_eq!(usable_reported_name(Some("unknown".into())), None);
        assert_eq!(usable_reported_name(Some("localhost".into())), None);
        assert_eq!(usable_reported_name(Some("   ".into())), None);
        assert_eq!(usable_reported_name(None), None);

        assert_eq!(
            usable_reported_name(Some("  Sam's MacBook Pro  ".into())),
            Some("Sam's MacBook Pro".to_string()),
        );
    }

    #[test]
    fn a_device_never_calls_itself_unknown() {
        let dir = TempDir::new().expect("temp dir");
        let name = identity_in(&dir).device_name();

        assert!(!name.trim().is_empty());
        assert!(
            !UNHELPFUL_NAMES.contains(&name.trim().to_lowercase().as_str()),
            "a fresh identity named itself {name:?}, which is a placeholder rather than a name",
        );
    }
}
