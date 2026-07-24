// engine/src/tls.rs
use rustls::pki_types::CertificateDer;
use std::io::Cursor;

#[derive(Debug)]
pub struct TrustedCerts {
    trusted_cert_ders: Vec<Vec<u8>>,
}

impl TrustedCerts {
    pub fn new(trusted_cert_pems: &[String]) -> Self {
        let mut trusted_cert_ders = Vec::new();
        for pem in trusted_cert_pems {
            let mut cursor = Cursor::new(pem.as_bytes());
            if let Ok(certs) = rustls_pemfile::certs(&mut cursor)
                .collect::<Result<Vec<CertificateDer<'static>>, _>>()
            {
                for cert in certs {
                    trusted_cert_ders.push(cert.to_vec());
                }
            }
        }
        Self { trusted_cert_ders }
    }

    pub fn is_trusted(&self, end_entity: &CertificateDer<'_>) -> bool {
        if self.trusted_cert_ders.is_empty() {
            return true;
        }
        let presented = end_entity.as_ref();
        self.trusted_cert_ders
            .iter()
            .any(|trusted| trusted.as_slice() == presented)
    }
}

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