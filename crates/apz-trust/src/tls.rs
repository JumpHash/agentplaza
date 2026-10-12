//! The hub's TLS: a self-signed certificate made on first start, its SHA-256 fingerprint (the pin
//! join tokens carry), a rustls server config, and a client config that trusts exactly the pin.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::ring::default_provider;
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error, ServerConfig, SignatureScheme};

/// A PEM certificate and its PEM private key.
pub struct CertPair {
    pub cert_pem: String,
    pub key_pem: String,
}

/// A self-signed certificate for `names` (DNS names or IP addresses as text).
pub fn self_signed(names: &[String]) -> Result<CertPair, String> {
    let key = rcgen::generate_simple_self_signed(names.to_vec()).map_err(|e| e.to_string())?;
    Ok(CertPair { cert_pem: key.cert.pem(), key_pem: key.key_pair.serialize_pem() })
}

/// The first certificate in `cert_pem`, as DER.
fn first_cert(cert_pem: &str) -> Result<CertificateDer<'static>, String> {
    let mut rd = cert_pem.as_bytes();
    rustls_pemfile::certs(&mut rd)
        .next()
        .ok_or_else(|| "no certificate in PEM".to_string())?
        .map_err(|e| e.to_string())
}

/// SHA-256 of the first certificate in `cert_pem` (DER).
pub fn fingerprint(cert_pem: &str) -> Result<[u8; 32], String> {
    Ok(crate::sha256(first_cert(cert_pem)?.as_ref()))
}

/// A rustls server config serving `pair`.
pub fn server_config(pair: &CertPair) -> Result<Arc<ServerConfig>, String> {
    let mut cert_rd = pair.cert_pem.as_bytes();
    let chain = rustls_pemfile::certs(&mut cert_rd)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if chain.is_empty() {
        return Err("no certificate in PEM".into());
    }
    let mut key_rd = pair.key_pem.as_bytes();
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_rd)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no private key in PEM".to_string())?;
    let config = ServerConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(config))
}

/// The signature algorithms of the ring provider, used by both the verifier and its advertised
/// schemes.
fn algorithms() -> WebPkiSupportedAlgorithms {
    default_provider().signature_verification_algorithms
}

/// Accepts a server certificate only if its end-entity DER hashes to the pin.
#[derive(Debug)]
struct Pinned([u8; 32]);

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if crate::ct_eq(&crate::sha256(end_entity.as_ref()), &self.0) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(Error::General("certificate does not match the pin".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        algorithms().supported_schemes()
    }
}

/// A rustls client config that accepts a server only if its end-entity certificate's SHA-256
/// equals `pin` (no CA, no hostname check — the pin is the identity).
pub fn pinned_client_config(pin: [u8; 32]) -> Arc<ClientConfig> {
    let config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned(pin)))
        .with_no_client_auth();
    Arc::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    /// Runs a handshake over a connected socket pair; `Ok` only if the client completes it.
    fn handshake(pair: &CertPair, pin: [u8; 32]) -> Result<(), String> {
        let server_cfg = server_config(pair)?;
        let client_cfg = pinned_client_config(pin);
        let (mut client_sock, mut server_sock) = UnixStream::pair().map_err(|e| e.to_string())?;

        let server = std::thread::spawn(move || -> Result<(), String> {
            let mut conn = rustls::ServerConnection::new(server_cfg).map_err(|e| e.to_string())?;
            conn.complete_io(&mut server_sock).map_err(|e| e.to_string())?;
            Ok(())
        });

        let mut client = rustls::ClientConnection::new(
            client_cfg,
            ServerName::try_from("localhost").map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let client_result = client.complete_io(&mut client_sock).map(|_| ()).map_err(|e| e.to_string());
        if client_result.is_ok() {
            // Flush the client's final flight (its Finished message) before the server reads it.
            while client.wants_write() {
                client.write_tls(&mut client_sock).map_err(|e| e.to_string())?;
            }
        } else {
            // Closing the socket ends the server's side of a failed handshake.
            drop(client_sock);
        }
        let server_result = server.join().map_err(|_| "server thread panicked".to_string())?;

        client_result?;
        server_result
    }

    #[test]
    fn fingerprint_is_stable_and_differs_per_certificate() {
        let first = self_signed(&["localhost".to_string()]).unwrap();
        let second = self_signed(&["localhost".to_string()]).unwrap();
        assert_eq!(fingerprint(&first.cert_pem).unwrap(), fingerprint(&first.cert_pem).unwrap());
        assert_ne!(fingerprint(&first.cert_pem).unwrap(), fingerprint(&second.cert_pem).unwrap());
    }

    #[test]
    fn server_config_builds_from_a_self_signed_pair() {
        let pair = self_signed(&["localhost".to_string()]).unwrap();
        server_config(&pair).unwrap();
    }

    #[test]
    fn handshake_completes_with_the_right_pin() {
        let pair = self_signed(&["localhost".to_string()]).unwrap();
        let pin = fingerprint(&pair.cert_pem).unwrap();
        handshake(&pair, pin).unwrap();
    }

    #[test]
    fn handshake_fails_with_a_wrong_pin() {
        let pair = self_signed(&["localhost".to_string()]).unwrap();
        let wrong = crate::sha256(b"not the hub's certificate");
        assert!(handshake(&pair, wrong).is_err());
    }
}
