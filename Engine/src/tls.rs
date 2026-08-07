// Engine/src/tls.rs
use rustls::pki_types::CertificateDer;
use std::io::Cursor;
use anyhow::Result;
use rustls::pki_types::PrivateKeyDer;
use std::sync::{Arc, RwLock};

/// The set of peer certificates this device has pinned through pairing.
///
/// Shared and reloadable: the running TLS server holds one of these, and
/// completing a pairing must take effect immediately rather than at the next
/// restart. An empty store trusts nobody.
#[derive(Debug, Clone, Default)]
pub struct TrustStore {
    certs: Arc<RwLock<Vec<Vec<u8>>>>,
}

impl TrustStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a point-in-time store from PEM strings.
    pub fn from_pems(pems: &[String]) -> Self {
        let store = Self::new();
        store.replace_with(pems);
        store
    }

    /// Reads every pinned certificate from disk, replacing the current set.
    pub fn reload(&self, storage_dir: &str) -> Result<()> {
        let pems = crate::storage::load_all_trusted_certs(storage_dir)?;
        self.replace_with(&pems);
        Ok(())
    }

    fn replace_with(&self, pems: &[String]) {
        let mut ders = Vec::new();
        for pem in pems {
            let mut cursor = Cursor::new(pem.as_bytes());
            if let Ok(certs) = rustls_pemfile::certs(&mut cursor)
                .collect::<Result<Vec<CertificateDer<'static>>, _>>()
            {
                for cert in certs {
                    ders.push(cert.to_vec());
                }
            }
        }
        *self.certs.write().unwrap() = ders;
    }

    /// Exact-match against a pinned certificate.
    ///
    /// Fails closed. An empty store means no device has been paired yet, which
    /// is precisely when nothing should be trusted.
    pub fn is_trusted_der(&self, presented: &[u8]) -> bool {
        self.certs
            .read()
            .unwrap()
            .iter()
            .any(|trusted| trusted.as_slice() == presented)
    }

    pub fn is_trusted(&self, end_entity: &CertificateDer<'_>) -> bool {
        self.is_trusted_der(end_entity.as_ref())
    }

    pub fn is_empty(&self) -> bool {
        self.certs.read().unwrap().is_empty()
    }
}

pub fn load_certs_and_key(
    cert_pem: &str,
    key_pem: &str,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let mut cert_cursor = Cursor::new(cert_pem.as_bytes());
    let mut key_cursor = Cursor::new(key_pem.as_bytes());

    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_cursor)
        .collect::<Result<Vec<_>, _>>()?;

    let key = PrivateKeyDer::from(
        rustls_pemfile::private_key(&mut key_cursor)?
            .ok_or_else(|| anyhow::anyhow!("No private key found in PEM"))?,
    );

    Ok((certs, key))
}

/// Fills in the rustls verifier methods that are the same for both directions.
///
/// `#[macro_export]` is the only way to use a macro across modules, and it puts
/// it at the crate root whether that is wanted or not - so this is hidden, like
/// the modules that use it. It is internal plumbing, not something to call.
#[doc(hidden)]
#[macro_export]
macro_rules! impl_tls_verifier_methods {
    () => {
        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &rustls::pki_types::CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &rustls::pki_types::CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            vec![
                rustls::SignatureScheme::ED25519,
                rustls::SignatureScheme::RSA_PSS_SHA256,
                rustls::SignatureScheme::RSA_PSS_SHA384,
                rustls::SignatureScheme::RSA_PSS_SHA512,
                rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
                rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            ]
        }
    };
}
