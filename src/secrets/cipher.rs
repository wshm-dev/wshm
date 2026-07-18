//! AES-256-GCM seal/open over the master key.
//!
//! The master key is loaded once at startup (32 bytes from
//! `WSHM_MASTER_KEY`). Each plaintext is encrypted with a fresh 12-byte
//! random nonce. AAD bytes bind the ciphertext to a logical identifier
//! (`scope|slug|key`) so an attacker cannot copy a ciphertext from one
//! row into another and have the GCM tag still validate.
//!
//! On-disk record layout:
//!   nonce      BLOB(12)  random per-write
//!   ciphertext BLOB(N+16) plaintext + 16-byte GCM tag
//!   aad        BLOB      "scope|slug|key" UTF-8 (nullable for global keys)

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{anyhow, bail, Context, Result};
use rand::RngCore;

/// 32-byte AES-256 key. Construct via [`MasterKey::from_hex`].
pub struct MasterKey([u8; 32]);

impl MasterKey {
    pub fn from_hex(hex_str: &str) -> Result<Self> {
        let bytes = hex::decode(hex_str.trim()).context("master key is not valid hex")?;
        if bytes.len() != 32 {
            bail!(
                "master key must be 32 bytes (64 hex chars), got {} bytes",
                bytes.len()
            );
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }

    /// Generate a fresh random 32-byte key (used by the bootstrap script).
    pub fn generate() -> Self {
        let mut arr = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut arr);
        Self(arr)
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

/// GCM cipher wrapping a [`MasterKey`].
pub struct Cipher {
    inner: Aes256Gcm,
}

impl Cipher {
    pub fn new(key: &MasterKey) -> Self {
        let k = Key::<Aes256Gcm>::from_slice(&key.0);
        Self {
            inner: Aes256Gcm::new(k),
        }
    }

    /// Encrypt `plaintext` with a fresh random nonce. Returns
    /// `(nonce_12B, ciphertext_with_tag)`.
    pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .inner
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|e| anyhow!("AES-GCM encrypt failed: {e}"))?;
        Ok((nonce_bytes.to_vec(), ciphertext))
    }

    pub fn open(&self, nonce: &[u8], ciphertext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
        if nonce.len() != 12 {
            bail!("nonce must be 12 bytes, got {}", nonce.len());
        }
        let n = Nonce::from_slice(nonce);
        self.inner
            .decrypt(
                n,
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|e| anyhow!("AES-GCM decrypt failed: {e}"))
    }

    /// Decrypt with one or more candidate AADs — first one that
    /// validates wins. Used by [`SecretStore::get`] to roll forward
    /// ciphertexts that were sealed with the legacy `"scope|slug|key"`
    /// AAD format (replaced by length-prefixed segments). Callers
    /// should re-seal the row with the canonical AAD on the next
    /// write so the legacy fallback eventually disappears.
    pub fn open_with_aads(
        &self,
        nonce: &[u8],
        ciphertext: &[u8],
        aads: &[&[u8]],
    ) -> Result<Vec<u8>> {
        let mut last_err: Option<anyhow::Error> = None;
        for aad in aads {
            match self.open(nonce, ciphertext, aad) {
                Ok(pt) => return Ok(pt),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("no AAD candidates supplied")))
    }
}

/// Legacy AAD shape `"scope|slug|key"` — only used as a fallback when
/// decrypting rows sealed before the length-prefixed format landed.
/// Do NOT use for new writes (ambiguous; see `aad_for` doc).
pub fn aad_for_legacy(scope: &str, slug: Option<&str>, key: &str) -> Vec<u8> {
    let s = format!("{}|{}|{}", scope, slug.unwrap_or(""), key);
    s.into_bytes()
}

/// Build the AAD bytes for a given record from its logical identity.
///
/// Each segment is length-prefixed (4-byte big-endian) so that no
/// combination of scope/slug/key collides with any other. The previous
/// `"{scope}|{slug}|{key}"` shape was ambiguous (e.g. `slug="a|b"
/// key="c"` and `slug="a" key="b|c"` produced identical AAD), letting
/// an attacker with raw DB write access swap ciphertexts between two
/// rows whose AAD coincidentally matched.
pub fn aad_for(scope: &str, slug: Option<&str>, key: &str) -> Vec<u8> {
    let scope_b = scope.as_bytes();
    let slug_b = slug.unwrap_or("").as_bytes();
    let key_b = key.as_bytes();
    let mut out = Vec::with_capacity(12 + scope_b.len() + slug_b.len() + key_b.len());
    out.extend_from_slice(&(scope_b.len() as u32).to_be_bytes());
    out.extend_from_slice(scope_b);
    out.extend_from_slice(&(slug_b.len() as u32).to_be_bytes());
    out.extend_from_slice(slug_b);
    out.extend_from_slice(&(key_b.len() as u32).to_be_bytes());
    out.extend_from_slice(key_b);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let k = MasterKey::generate();
        let c = Cipher::new(&k);
        let aad = aad_for("global", None, "github_token");
        let (nonce, ct) = c.seal(b"ghp_secret", &aad).unwrap();
        let pt = c.open(&nonce, &ct, &aad).unwrap();
        assert_eq!(pt, b"ghp_secret");
    }

    #[test]
    fn aad_mismatch_fails() {
        let k = MasterKey::generate();
        let c = Cipher::new(&k);
        let aad = aad_for("global", None, "github_token");
        let (nonce, ct) = c.seal(b"ghp_secret", &aad).unwrap();
        // Different AAD must fail (defends against row-substitution attack).
        let bad_aad = aad_for("global", None, "anthropic_key");
        assert!(c.open(&nonce, &ct, &bad_aad).is_err());
    }

    /// Regression test for the unescaped-`|` substitution attack: the
    /// AAD format must distinguish `slug="a|b"` from `slug="a"` even
    /// when the trailing key includes a `|`.
    #[test]
    fn aad_segments_are_unambiguous() {
        let a = aad_for("repo", Some("a|b"), "key");
        let b = aad_for("repo", Some("a"), "b|key");
        assert_ne!(a, b, "AAD must not collide across segment splits");
    }
}
