//! Web Push message encryption (RFC 8291, `aes128gcm` from RFC 8188) and
//! VAPID authentication (RFC 8292), on `ring`.

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use ring::{aead, agreement, hmac};

/// The record size put in the header; one record holds any payload we send.
const RECORD_SIZE: u32 = 4096;
/// How long a VAPID token is good for (the spec allows at most 24 h).
const JWT_TTL_SECS: u64 = 12 * 3600;

/// The server's VAPID key pair, kept as PKCS#8 in `vapid.key` (mode 0600).
pub struct Vapid {
    key: EcdsaKeyPair,
}

impl Vapid {
    /// Loads `path`, or makes a key and writes it there on first start.
    pub fn load_or_create(path: &Path) -> std::io::Result<Vapid> {
        let rng = SystemRandom::new();
        let bad = |e: ring::error::Unspecified| std::io::Error::other(format!("vapid key: {e}"));
        let pkcs8 = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                    .map_err(bad)?;
                crate::store::atomic_write(path, doc.as_ref())?;
                doc.as_ref().to_vec()
            }
            Err(e) => return Err(e),
        };
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pkcs8, &rng)
            .map_err(|e| std::io::Error::other(format!("vapid key {}: {e}", path.display())))?;
        Ok(Vapid { key })
    }

    /// The uncompressed public key, URL-safe base64.
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.public_key().as_ref())
    }

    /// The `Authorization` header for a push to `endpoint`'s origin.
    pub fn authorization(&self, endpoint: &str, subject: &str, now_secs: u64) -> Option<String> {
        let aud = origin(endpoint)?;
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = serde_json::json!({
            "aud": aud,
            "exp": now_secs + JWT_TTL_SECS,
            "sub": subject,
        });
        let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signed = format!("{header}.{claims}");
        let sig = self
            .key
            .sign(&SystemRandom::new(), signed.as_bytes())
            .ok()?;
        let sig = URL_SAFE_NO_PAD.encode(sig.as_ref());
        Some(format!("vapid t={signed}.{sig}, k={}", self.public_key()))
    }
}

/// `scheme://host[:port]` of a URL.
fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
}

#[derive(Debug, PartialEq, Eq)]
pub struct CryptoError(pub &'static str);

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut ctx = hmac::Context::with_key(&hmac::Key::new(hmac::HMAC_SHA256, key));
    for p in parts {
        ctx.update(p);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(ctx.sign().as_ref());
    out
}

/// The content key and nonce (RFC 8291 section 3.4, RFC 8188 section 2.2).
fn derive(
    ecdh_secret: &[u8],
    auth: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8],
) -> ([u8; 16], [u8; 12]) {
    let prk_key = hmac_sha256(auth, &[ecdh_secret]);
    let ikm = hmac_sha256(&prk_key, &[b"WebPush: info\0", ua_public, as_public, &[1]]);
    let prk = hmac_sha256(salt, &[&ikm]);
    let mut cek = [0u8; 16];
    cek.copy_from_slice(&hmac_sha256(&prk, &[b"Content-Encoding: aes128gcm\0", &[1]])[..16]);
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&hmac_sha256(&prk, &[b"Content-Encoding: nonce\0", &[1]])[..12]);
    (cek, nonce)
}

/// Encrypts `plaintext` for a subscription's `p256dh` and `auth` keys (raw
/// bytes). The result is the request body, with the `aes128gcm` header.
pub fn encrypt(plaintext: &[u8], ua_public: &[u8], auth: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let rng = SystemRandom::new();
    let mut salt = [0u8; 16];
    rng.fill(&mut salt)
        .map_err(|_| CryptoError("no randomness"))?;
    let as_key = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng)
        .map_err(|_| CryptoError("cannot make a key"))?;
    let as_public = as_key
        .compute_public_key()
        .map_err(|_| CryptoError("cannot make a key"))?;
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, ua_public);
    let (cek, nonce) = agreement::agree_ephemeral(as_key, &peer, |secret| {
        derive(secret, auth, ua_public, as_public.as_ref(), &salt)
    })
    .map_err(|_| CryptoError("bad p256dh key"))?;
    // One record: the payload, then the last-record delimiter.
    let mut record = Vec::with_capacity(plaintext.len() + 17);
    record.extend_from_slice(plaintext);
    record.push(2);
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_128_GCM, &cek).map_err(|_| CryptoError("bad key"))?,
    );
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::empty(),
        &mut record,
    )
    .map_err(|_| CryptoError("cannot encrypt"))?;
    if record.len() > RECORD_SIZE as usize {
        return Err(CryptoError("payload too large"));
    }
    let mut body = Vec::with_capacity(21 + 65 + record.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(as_public.as_ref().len() as u8);
    body.extend_from_slice(as_public.as_ref());
    body.extend_from_slice(&record);
    Ok(body)
}

/// A browser's side: decrypts a body made by [`encrypt`] (tests and mocks).
pub fn decrypt(
    body: &[u8],
    ua_key: agreement::EphemeralPrivateKey,
    ua_public: &[u8],
    auth: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let short = CryptoError("short body");
    let salt = body.get(..16).ok_or(short)?;
    let idlen = *body.get(20).ok_or(CryptoError("short body"))? as usize;
    let as_public = body.get(21..21 + idlen).ok_or(CryptoError("short body"))?;
    let mut record = body[21 + idlen..].to_vec();
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, as_public);
    let (cek, nonce) = agreement::agree_ephemeral(ua_key, &peer, |secret| {
        derive(secret, auth, ua_public, as_public, salt)
    })
    .map_err(|_| CryptoError("bad key id"))?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_128_GCM, &cek).map_err(|_| CryptoError("bad key"))?,
    );
    let plain = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::empty(),
            &mut record,
        )
        .map_err(|_| CryptoError("cannot decrypt"))?;
    match plain.iter().rposition(|&b| b != 0) {
        Some(i) if plain[i] == 2 => Ok(plain[..i].to_vec()),
        _ => Err(CryptoError("no last-record delimiter")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    fn browser() -> (agreement::EphemeralPrivateKey, Vec<u8>, [u8; 16]) {
        let rng = SystemRandom::new();
        let key = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let public = key.compute_public_key().unwrap().as_ref().to_vec();
        let mut auth = [0u8; 16];
        rng.fill(&mut auth).unwrap();
        (key, public, auth)
    }

    #[test]
    fn a_browser_can_decrypt_what_the_server_encrypts() {
        let (key, public, auth) = browser();
        let body = encrypt(b"{\"kind\":\"permission\"}", &public, &auth).unwrap();
        assert_eq!(&body[16..20], &4096u32.to_be_bytes());
        assert_eq!(body[20], 65);
        let plain = decrypt(&body, key, &public, &auth).unwrap();
        assert_eq!(plain, b"{\"kind\":\"permission\"}");
    }

    /// RFC 8291 appendix A: from the shared secret to the ciphertext.
    #[test]
    fn the_rfc_8291_example_gives_the_same_key_nonce_and_ciphertext() {
        let d = |s: &str| URL_SAFE_NO_PAD.decode(s.replace(' ', "")).unwrap();
        let as_public = d("BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIg\
            Dll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8");
        let ua_public = d("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-\
            JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let ecdh = d("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs");
        let (cek, nonce) = derive(
            &ecdh,
            &d("BTBZMqHH6r4Tts7J_aSIgg"),
            &ua_public,
            &as_public,
            &d("DGv6ra1nlYgDCS1FRnbzlw"),
        );
        assert_eq!(cek.to_vec(), d("oIhVW04MRdy2XN9CiKLxTg"));
        assert_eq!(nonce.to_vec(), d("4h_95klXJ5E_qnoN"));
        let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).unwrap());
        let mut record = d("V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24");
        record.push(2);
        key.seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::empty(),
            &mut record,
        )
        .unwrap();
        assert_eq!(
            record,
            d("8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ")
        );
    }

    #[test]
    fn the_wrong_auth_secret_does_not_decrypt() {
        let (key, public, auth) = browser();
        let body = encrypt(b"x", &public, &auth).unwrap();
        assert!(decrypt(&body, key, &public, &[0u8; 16]).is_err());
    }

    #[test]
    fn a_bad_p256dh_key_is_refused() {
        assert_eq!(
            encrypt(b"x", &[4u8; 10], &[0u8; 16]),
            Err(CryptoError("bad p256dh key"))
        );
    }

    #[test]
    fn the_vapid_key_is_made_once_with_mode_0600_and_signs_a_valid_jwt() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("vapid.key");
        let v = Vapid::load_or_create(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let again = Vapid::load_or_create(&path).unwrap();
        assert_eq!(v.public_key(), again.public_key());

        let h = v
            .authorization(
                "https://push.example.com/send/abc?x=1",
                "mailto:a@b.c",
                1000,
            )
            .unwrap();
        let rest = h.strip_prefix("vapid t=").unwrap();
        let (jwt, k) = rest.split_once(", k=").unwrap();
        assert_eq!(k, v.public_key());
        let (signed, sig) = jwt.rsplit_once('.').unwrap();
        let claims = signed.split('.').nth(1).unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://push.example.com");
        assert_eq!(claims["exp"], 1000 + JWT_TTL_SECS);
        let public = URL_SAFE_NO_PAD.decode(k).unwrap();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public)
            .verify(signed.as_bytes(), &URL_SAFE_NO_PAD.decode(sig).unwrap())
            .unwrap();
    }
}
