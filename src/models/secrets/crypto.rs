//! Envelope encryption for login secrets, with XChaCha20-Poly1305.
//!
//! Each organisation has its own data key (DEK). The DEK is stored wrapped
//! (encrypted) by the key-encryption key (KEK) from `SECRETS_KEK`. During a
//! rotation `SECRETS_KEK_PREVIOUS` still unwraps the DEKs not yet moved to the
//! new KEK (see `org_keys::Model::rewrap`). Fields are
//! encrypted with the DEK. Every encryption binds associated data naming what
//! the value belongs to, so a ciphertext copied to another login, field,
//! organisation or key version fails to open.
//!
//! Keys and plaintext are only ever held in `secrecy` types, which never
//! print and are zeroed when dropped.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, Generate, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use loco_rs::prelude::*;
use secrecy::{ExposeSecret, SecretBox, SecretString};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;

/// A 256-bit key, never printed, zeroed on drop.
pub type Key = SecretBox<[u8; KEY_LEN]>;

/// The KEK version when `SECRETS_KEK_VERSION` is not set. Stored with each
/// wrapped DEK.
pub const DEFAULT_KEK_VERSION: i32 = 1;

/// The key development and tests use. Production refuses to start with it.
pub const DEV_KEK: &str = "EUqaJ5SN03zOzpn2Ec9MZ7fVH4UrZP4wo4/lTTAVwlE=";

/// The `settings:` keys the KEKs are read from. Production fills the
/// optional ones from the environment, so unset arrives as an empty string.
#[derive(serde::Deserialize)]
struct KekSettings {
    secrets_kek: SecretString,
    #[serde(default)]
    secrets_kek_version: Option<String>,
    #[serde(default)]
    secrets_kek_previous: Option<SecretString>,
    #[serde(default)]
    secrets_kek_previous_version: Option<String>,
}

/// Ciphertext (with its tag) and the random nonce it was sealed with.
pub struct Sealed {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
}

/// The key-encryption keys, read once at boot: the current one, which wraps
/// every new DEK, and during a rotation the previous one, which only unwraps
/// DEKs not yet moved to the current one.
pub struct Kek {
    key: Key,
    pub version: i32,
    previous: Option<(Key, i32)>,
}

/// Versions only, never the keys.
impl std::fmt::Debug for Kek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kek")
            .field("version", &self.version)
            .field("previous_version", &self.previous_version())
            .finish_non_exhaustive()
    }
}

/// Decodes one KEK from base64. `name` is the variable it came from.
fn decode_key(name: &str, encoded: &str, production: bool) -> Result<Key> {
    let refuse = || {
        Error::string(&format!(
            "{name} must be base64 of 32 random bytes (openssl rand -base64 32)"
        ))
    };
    let bytes = SecretBox::new(Box::new(
        STANDARD.decode(encoded.trim()).map_err(|_| refuse())?,
    ));
    let key: [u8; KEY_LEN] = bytes
        .expose_secret()
        .as_slice()
        .try_into()
        .map_err(|_| refuse())?;
    if production && encoded.trim() == DEV_KEK {
        return Err(Error::string(&format!(
            "{name} is the development key; set a real one in production"
        )));
    }
    Ok(SecretBox::new(Box::new(key)))
}

/// Reads a version number; empty or missing is `None`.
fn parse_version(name: &str, value: Option<&str>) -> Result<Option<i32>> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(text) => match text.parse::<i32>() {
            Ok(version) if version > 0 => Ok(Some(version)),
            _ => Err(Error::string(&format!(
                "{name} must be a whole number above 0"
            ))),
        },
    }
}

impl Kek {
    /// Reads the current KEK from base64. In production it must not be the
    /// development and test key.
    ///
    /// # Errors
    /// When it is not base64 of exactly 32 bytes, or production is given
    /// the development key.
    pub fn from_base64(encoded: &str, version: i32, production: bool) -> Result<Self> {
        Ok(Self {
            key: decode_key("SECRETS_KEK", encoded, production)?,
            version,
            previous: None,
        })
    }

    /// Adds the previous KEK, used only to unwrap DEKs still on `version`.
    ///
    /// # Errors
    /// The same as [`Self::from_base64`], when the previous key is the same
    /// as the current one, or its version is not lower.
    pub fn with_previous(mut self, encoded: &str, version: i32, production: bool) -> Result<Self> {
        let key = decode_key("SECRETS_KEK_PREVIOUS", encoded, production)?;
        if key.expose_secret() == self.key.expose_secret() {
            return Err(Error::string(
                "SECRETS_KEK_PREVIOUS is the same key as SECRETS_KEK",
            ));
        }
        if version >= self.version {
            return Err(Error::string(
                "SECRETS_KEK_VERSION must be higher than SECRETS_KEK_PREVIOUS_VERSION",
            ));
        }
        self.previous = Some((key, version));
        Ok(self)
    }

    /// Reads the keys from `settings:`. Called once at boot, so a missing
    /// or bad key stops the app from starting.
    ///
    /// # Errors
    /// See [`Self::from_base64`] and [`Self::with_previous`], and when only
    /// one of the previous key and its version is set.
    pub fn from_context(ctx: &AppContext) -> Result<Self> {
        Self::from_settings(
            &ctx.config.settings()?,
            ctx.environment == loco_rs::environment::Environment::Production,
        )
    }

    fn from_settings(settings: &KekSettings, production: bool) -> Result<Self> {
        let version = parse_version(
            "SECRETS_KEK_VERSION",
            settings.secrets_kek_version.as_deref(),
        )?
        .unwrap_or(DEFAULT_KEK_VERSION);
        let kek = Self::from_base64(settings.secrets_kek.expose_secret(), version, production)?;
        let previous = settings
            .secrets_kek_previous
            .as_ref()
            .map(|key| key.expose_secret().trim())
            .filter(|key| !key.is_empty());
        let previous_version = parse_version(
            "SECRETS_KEK_PREVIOUS_VERSION",
            settings.secrets_kek_previous_version.as_deref(),
        )?;
        match (previous, previous_version) {
            (Some(key), Some(previous_version)) => {
                kek.with_previous(key, previous_version, production)
            }
            (Some(_), None) => Err(Error::string(
                "SECRETS_KEK_PREVIOUS is set without SECRETS_KEK_PREVIOUS_VERSION",
            )),
            (None, Some(_)) => Err(Error::string(
                "SECRETS_KEK_PREVIOUS_VERSION is set without SECRETS_KEK_PREVIOUS",
            )),
            (None, None) => Ok(kek),
        }
    }

    /// The KEK loaded at boot.
    ///
    /// # Errors
    /// When the app was started without one.
    pub fn get(ctx: &AppContext) -> ModelResult<std::sync::Arc<Self>> {
        ctx.shared_store
            .get::<std::sync::Arc<Self>>()
            .ok_or_else(|| ModelError::msg("the secrets key was not loaded"))
    }

    /// The previous KEK's version, while one is set.
    #[must_use]
    pub fn previous_version(&self) -> Option<i32> {
        self.previous.as_ref().map(|(_, version)| *version)
    }

    /// Whether a DEK wrapped by KEK `version` can be unwrapped.
    #[must_use]
    pub fn has_version(&self, version: i32) -> bool {
        version == self.version || self.previous_version() == Some(version)
    }

    fn key_for(&self, version: i32) -> ModelResult<&Key> {
        if version == self.version {
            return Ok(&self.key);
        }
        match &self.previous {
            Some((key, previous)) if *previous == version => Ok(key),
            _ => Err(ModelError::Message(format!(
                "data key was wrapped by KEK v{version}, which is not loaded"
            ))),
        }
    }

    fn wrap_aad(org_id: i64, kek_version: i32) -> String {
        format!("org:{org_id}|kek:{kek_version}")
    }

    /// Encrypts an organisation's DEK with the current KEK.
    ///
    /// # Errors
    /// See [`seal`].
    pub fn wrap(&self, org_id: i64, dek: &Key) -> ModelResult<Sealed> {
        seal(
            &self.key,
            dek.expose_secret(),
            &Self::wrap_aad(org_id, self.version),
        )
    }

    /// Decrypts an organisation's DEK with the KEK of `kek_version`.
    ///
    /// # Errors
    /// When that KEK is not loaded, or the DEK was wrapped for another
    /// organisation or KEK, or was changed.
    pub fn unwrap(
        &self,
        org_id: i64,
        kek_version: i32,
        wrapped: &[u8],
        nonce: &[u8],
    ) -> ModelResult<Key> {
        let bytes = open_bytes(
            self.key_for(kek_version)?,
            wrapped,
            nonce,
            &Self::wrap_aad(org_id, kek_version),
        )?;
        let key: [u8; KEY_LEN] = bytes
            .expose_secret()
            .as_slice()
            .try_into()
            .map_err(|_| ModelError::msg("data key has the wrong length"))?;
        Ok(SecretBox::new(Box::new(key)))
    }
}

/// A new random key.
#[must_use]
pub fn random_key() -> Key {
    SecretBox::new(Box::new(chacha20poly1305::Key::generate().into()))
}

/// The associated data for one encrypted field of a login.
#[must_use]
pub fn field_aad(org_id: i64, login_id: i64, field: &str, dek_version: i32) -> String {
    format!("org:{org_id}|login:{login_id}|field:{field}|dek:{dek_version}")
}

fn cipher(key: &Key) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(&(*key.expose_secret()).into())
}

/// Encrypts `plaintext` under `key` with a fresh random nonce, bound to `aad`.
///
/// # Errors
/// Only for messages far over any size the app accepts.
pub fn seal(key: &Key, plaintext: &[u8], aad: &str) -> ModelResult<Sealed> {
    let nonce = XNonce::generate();
    let ciphertext = cipher(key)
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| ModelError::msg("could not encrypt a secret"))?;
    Ok(Sealed {
        ciphertext,
        nonce: nonce.to_vec(),
    })
}

fn open_bytes(
    key: &Key,
    ciphertext: &[u8],
    nonce: &[u8],
    aad: &str,
) -> ModelResult<SecretBox<Vec<u8>>> {
    let nonce: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| ModelError::msg("nonce has the wrong length"))?;
    cipher(key)
        .decrypt(
            &nonce.into(),
            Payload {
                msg: ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .map(|bytes| SecretBox::new(Box::new(bytes)))
        .map_err(|_| ModelError::msg("could not decrypt a secret"))
}

/// Decrypts a field sealed with [`seal`] under the same key and `aad`.
///
/// # Errors
/// When the key, nonce or associated data differ, or the ciphertext changed.
pub fn open(key: &Key, ciphertext: &[u8], nonce: &[u8], aad: &str) -> ModelResult<SecretString> {
    let bytes = open_bytes(key, ciphertext, nonce, aad)?;
    let text = std::str::from_utf8(bytes.expose_secret())
        .map_err(|_| ModelError::msg("a secret is not text"))?;
    Ok(SecretString::from(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_round_trips_only_with_the_same_associated_data() {
        let key = random_key();
        let aad = field_aad(1, 7, "password", 1);
        let sealed = seal(&key, b"example-pass-123", &aad).unwrap();
        assert_eq!(sealed.nonce.len(), NONCE_LEN);
        assert!(!sealed.ciphertext.windows(7).any(|w| w == b"example"));
        let opened = open(&key, &sealed.ciphertext, &sealed.nonce, &aad).unwrap();
        assert_eq!(opened.expose_secret(), "example-pass-123");

        for other in [
            field_aad(1, 8, "password", 1),
            field_aad(2, 7, "password", 1),
            field_aad(1, 7, "recovery", 1),
            field_aad(1, 7, "password", 2),
        ] {
            assert!(
                open(&key, &sealed.ciphertext, &sealed.nonce, &other).is_err(),
                "{other}"
            );
        }
        assert!(open(&random_key(), &sealed.ciphertext, &sealed.nonce, &aad).is_err());
        let mut tampered = sealed.ciphertext.clone();
        tampered[0] ^= 1;
        assert!(open(&key, &tampered, &sealed.nonce, &aad).is_err());
    }

    #[test]
    fn nonces_are_never_reused() {
        let key = random_key();
        let a = seal(&key, b"same", "x").unwrap();
        let b = seal(&key, b"same", "x").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    const V: i32 = DEFAULT_KEK_VERSION;

    fn other_key(byte: u8) -> String {
        STANDARD.encode([byte; KEY_LEN])
    }

    fn settings(
        version: Option<&str>,
        previous: Option<&str>,
        previous_version: Option<&str>,
    ) -> KekSettings {
        KekSettings {
            secrets_kek: SecretString::from(other_key(2)),
            secrets_kek_version: version.map(str::to_string),
            secrets_kek_previous: previous.map(SecretString::from),
            secrets_kek_previous_version: previous_version.map(str::to_string),
        }
    }

    #[test]
    fn a_dek_unwraps_only_for_its_organisation_and_kek_version() {
        let kek = Kek::from_base64(DEV_KEK, V, false).unwrap();
        let dek = random_key();
        let wrapped = kek.wrap(5, &dek).unwrap();
        let back = kek
            .unwrap(5, V, &wrapped.ciphertext, &wrapped.nonce)
            .unwrap();
        assert_eq!(back.expose_secret(), dek.expose_secret());
        assert!(kek
            .unwrap(6, V, &wrapped.ciphertext, &wrapped.nonce)
            .is_err());
        assert!(kek
            .unwrap(5, V + 1, &wrapped.ciphertext, &wrapped.nonce)
            .is_err());
    }

    #[test]
    fn the_kek_must_be_32_bytes_and_not_the_dev_key_in_production() {
        assert!(Kek::from_base64(DEV_KEK, V, false).is_ok());
        assert!(Kek::from_base64(DEV_KEK, V, true).is_err());
        assert!(Kek::from_base64("not base64!", V, false).is_err());
        assert!(Kek::from_base64(&STANDARD.encode([7u8; 16]), V, true).is_err());
        assert!(Kek::from_base64(&STANDARD.encode([7u8; 33]), V, true).is_err());
        assert!(Kek::from_base64(&STANDARD.encode([7u8; 32]), V, true).is_ok());
        assert!(Kek::from_base64("", V, true).is_err());
    }

    #[test]
    fn the_ring_unwraps_by_version_and_wraps_with_the_current_key() {
        let old = Kek::from_base64(DEV_KEK, 1, false).unwrap();
        let dek = random_key();
        let on_v1 = old.wrap(5, &dek).unwrap();

        let ring = Kek::from_base64(&other_key(2), 2, false)
            .unwrap()
            .with_previous(DEV_KEK, 1, false)
            .unwrap();
        assert_eq!(ring.previous_version(), Some(1));
        let back = ring.unwrap(5, 1, &on_v1.ciphertext, &on_v1.nonce).unwrap();
        assert_eq!(back.expose_secret(), dek.expose_secret());

        // New wraps use v2, which the old key alone cannot open.
        let on_v2 = ring.wrap(5, &dek).unwrap();
        let back = ring.unwrap(5, 2, &on_v2.ciphertext, &on_v2.nonce).unwrap();
        assert_eq!(back.expose_secret(), dek.expose_secret());
        assert!(old.unwrap(5, 2, &on_v2.ciphertext, &on_v2.nonce).is_err());
        // A row claiming the other version does not open either.
        assert!(ring.unwrap(5, 1, &on_v2.ciphertext, &on_v2.nonce).is_err());
        assert!(ring.unwrap(5, 2, &on_v1.ciphertext, &on_v1.nonce).is_err());
    }

    #[test]
    fn an_unknown_kek_version_is_a_clear_error() {
        let ring = Kek::from_base64(&other_key(2), 2, false)
            .unwrap()
            .with_previous(DEV_KEK, 1, false)
            .unwrap();
        let dek = random_key();
        let sealed = ring.wrap(5, &dek).unwrap();
        let Err(err) = ring.unwrap(5, 3, &sealed.ciphertext, &sealed.nonce) else {
            panic!("v3 is not loaded");
        };
        assert!(
            err.to_string().contains("KEK v3, which is not loaded"),
            "{err}"
        );
        assert!(!ring.has_version(3));
        assert!(ring.has_version(1) && ring.has_version(2));
    }

    #[test]
    fn the_previous_key_is_checked_like_the_current_one() {
        let current = || Kek::from_base64(&other_key(2), 2, true).unwrap();
        assert!(current().with_previous(&other_key(1), 1, true).is_ok());
        // The same key, or a version that is not lower, is refused.
        assert!(current().with_previous(&other_key(2), 1, true).is_err());
        assert!(current().with_previous(&other_key(1), 2, true).is_err());
        let Err(err) = current().with_previous(&other_key(1), 3, true) else {
            panic!("the previous version must be lower");
        };
        assert!(
            err.to_string()
                .contains("SECRETS_KEK_VERSION must be higher than SECRETS_KEK_PREVIOUS_VERSION"),
            "{err}"
        );
        assert!(current().with_previous(DEV_KEK, 1, true).is_err());
        assert!(current().with_previous("not base64!", 1, true).is_err());
        assert!(current()
            .with_previous(&STANDARD.encode([7u8; 16]), 1, true)
            .is_err());
    }

    #[test]
    fn settings_need_a_version_with_the_previous_key() {
        let previous = other_key(1);
        let ring =
            Kek::from_settings(&settings(Some("2"), Some(&previous), Some("1")), true).unwrap();
        assert_eq!((ring.version, ring.previous_version()), (2, Some(1)));

        let Err(err) = Kek::from_settings(&settings(Some("2"), Some(&previous), None), true) else {
            panic!("the previous key needs its version");
        };
        assert!(
            err.to_string().contains("SECRETS_KEK_PREVIOUS_VERSION"),
            "{err}"
        );
        assert!(Kek::from_settings(&settings(Some("2"), Some(&previous), Some("")), true).is_err());
        assert!(
            Kek::from_settings(&settings(Some("1"), Some(&previous), Some("1")), true).is_err()
        );
        assert!(Kek::from_settings(&settings(Some("two"), None, None), true).is_err());
        assert!(Kek::from_settings(&settings(Some("0"), None, None), true).is_err());

        // Unset (or empty, as production's config gives it) changes nothing.
        let today = Kek::from_settings(&settings(None, None, None), true).unwrap();
        assert_eq!((today.version, today.previous_version()), (1, None));
        let today = Kek::from_settings(&settings(Some(""), Some(""), Some("")), true).unwrap();
        assert_eq!((today.version, today.previous_version()), (1, None));
        // Swapped versions, or a stray previous version alone, are refused.
        assert!(
            Kek::from_settings(&settings(Some("1"), Some(&previous), Some("2")), true).is_err()
        );
        let Err(err) = Kek::from_settings(&settings(Some("2"), None, Some("1")), true) else {
            panic!("a previous version needs its key");
        };
        assert!(
            err.to_string().contains("without SECRETS_KEK_PREVIOUS"),
            "{err}"
        );
    }
}
