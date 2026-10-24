//! The signed JWTs of the store APIs (9.6): ES256 with a team's App Store Connect key (the `.p8`, a PKCS#8 P-256 key) and RS256
//! with a Google service account's key (a PKCS#8 RSA key). Both are signed with ring, as the GitHub App's JWT is
//! (`platforms/github.rs`).

use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
};
use serde_json::{Value, json};

/// An ES256 JWT: `kid` is the key's id in the header, `claims` the payload.
pub fn es256(pem: &str, kid: &str, claims: &Value) -> Result<String, String> {
    let input = signing_input(&json!({ "alg": "ES256", "kid": kid, "typ": "JWT" }), claims);
    let key = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        &der(pem)?,
        &SystemRandom::new(),
    )
    .map_err(|e| format!("the App Store Connect key is not a P-256 PKCS#8 key: {e}"))?;
    let signature = key
        .sign(&SystemRandom::new(), input.as_bytes())
        .map_err(|_| "signing the App Store Connect JWT failed".to_string())?;
    Ok(format!("{input}.{}", b64(signature.as_ref())))
}

/// An RS256 JWT with `claims` as its payload.
pub fn rs256(pem: &str, claims: &Value) -> Result<String, String> {
    let input = signing_input(&json!({ "alg": "RS256", "typ": "JWT" }), claims);
    let key = RsaKeyPair::from_pkcs8(&der(pem)?)
        .map_err(|e| format!("the service account's key is not an RSA PKCS#8 key: {e}"))?;
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        input.as_bytes(),
        &mut signature,
    )
    .map_err(|_| "signing the service account's JWT failed".to_string())?;
    Ok(format!("{input}.{}", b64(&signature)))
}

fn signing_input(header: &Value, claims: &Value) -> String {
    format!(
        "{}.{}",
        b64(header.to_string().as_bytes()),
        b64(claims.to_string().as_bytes())
    )
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The DER inside a PEM block.
fn der(pem: &str) -> Result<Vec<u8>, String> {
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .map(str::trim)
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .map_err(|e| format!("a store key is not PEM: {e}"))
}
