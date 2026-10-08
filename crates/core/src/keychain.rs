//! Tokens in the macOS Keychain, and the way in for tokens the Electron app left
//! behind.
//!
//! This replaces the safeStorage half of src/main/store.ts. The Electron app kept
//! each token in the vault file, encrypted by `safeStorage` with a key that lived in
//! the Keychain; this app keeps each token in the Keychain itself, as a generic
//! password per account (service [`TOKEN_SERVICE`], account = the account id), so
//! nothing secret sits in the vault at all.
//!
//! The migration reads what `safeStorage` wrote. On macOS that is Chromium's
//! OSCrypt format: the literal prefix `v10`, then AES-128-CBC with PKCS#7 padding
//! and an IV of sixteen spaces, under a key derived with PBKDF2-HMAC-SHA1 from the
//! password Electron keeps in the Keychain item "<app name> Safe Storage" (salt
//! `saltysalt`, 1003 iterations, 16 bytes). The crypto goes through CommonCrypto,
//! which ships in libSystem, so it costs no crate.

use std::ffi::{c_int, c_uint, c_void};
use std::sync::OnceLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use security_framework::passwords;

use crate::error::{Result, msg};
use crate::store::TokenStore;

/// The Keychain service every account token is filed under.
pub const TOKEN_SERVICE: &str = "cz.mares.reviewdeck.token";

/// The names Electron may have filed its Safe Storage key under: `app.getName()`,
/// which is the package name in a development build and the product name in a
/// packaged one.
const ELECTRON_APP_NAMES: [&str; 2] = ["reviewdeck", "Reviewdeck"];

/// What every `safeStorage` blob on macOS starts with.
const SAFE_STORAGE_PREFIX: &[u8] = b"v10";
const SAFE_STORAGE_SALT: &[u8] = b"saltysalt";
const SAFE_STORAGE_ITERATIONS: u32 = 1003;
const SAFE_STORAGE_KEY_LEN: usize = 16;
/// Chromium's OSCrypt uses a fixed IV of sixteen spaces.
const SAFE_STORAGE_IV: [u8; 16] = [b' '; 16];

/// `errSecItemNotFound`: there is no such Keychain item.
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// Account tokens in the login Keychain, one generic password per account.
pub struct KeychainTokens {
    service: String,
    /// The Electron Safe Storage passwords, read at most once per launch and only
    /// when there is something to migrate, because reading another app's item makes
    /// macOS ask the user.
    legacy_passwords: OnceLock<Vec<String>>,
}

impl KeychainTokens {
    /// The store the app uses: service [`TOKEN_SERVICE`].
    pub fn new() -> KeychainTokens {
        KeychainTokens::with_service(TOKEN_SERVICE)
    }

    /// A store under another service name, so a test can use a throwaway item.
    pub fn with_service(service: impl Into<String>) -> KeychainTokens {
        KeychainTokens {
            service: service.into(),
            legacy_passwords: OnceLock::new(),
        }
    }
}

impl Default for KeychainTokens {
    fn default() -> Self {
        KeychainTokens::new()
    }
}

impl TokenStore for KeychainTokens {
    fn get(&self, account_id: &str) -> Result<Option<String>> {
        match passwords::get_generic_password(&self.service, account_id) {
            Ok(bytes) => Ok(String::from_utf8(bytes).ok()),
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(error) => Err(msg(format!(
                "The token could not be read from the Keychain ({error})."
            ))),
        }
    }

    fn set(&self, account_id: &str, token: &str) -> Result<()> {
        passwords::set_generic_password(&self.service, account_id, token.as_bytes()).map_err(|_| {
            // Nowhere safe to put it, so it is not put anywhere.
            msg("Encrypted storage is unavailable, so the token cannot be saved safely.")
        })
    }

    fn delete(&self, account_id: &str) -> Result<()> {
        match passwords::delete_generic_password(&self.service, account_id) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(error) => Err(msg(format!(
                "The token could not be removed from the Keychain ({error})."
            ))),
        }
    }

    fn decrypt_legacy(&self, blob: &str) -> Option<String> {
        self.legacy_passwords
            .get_or_init(safe_storage_passwords)
            .iter()
            .find_map(|password| decrypt_safe_storage(blob, password))
    }
}

/// Every Electron Safe Storage password this machine holds for the app, in the
/// order they are worth trying.
///
/// Electron files the key under service "<name> Safe Storage" and account
/// "<name> Key"; builds before it added the suffix used the bare "<name>", and
/// Electron itself still falls back to that, so this does too. Both spellings of
/// the name are tried, because a development build and a packaged one name the app
/// differently.
pub fn safe_storage_passwords() -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for name in ELECTRON_APP_NAMES {
        let service = format!("{name} Safe Storage");
        for account in [format!("{name} Key"), name.to_string()] {
            if let Ok(bytes) = passwords::get_generic_password(&service, &account)
                && let Ok(password) = String::from_utf8(bytes)
                && !found.contains(&password)
            {
                found.push(password);
            }
        }
    }
    found
}

/// The first Electron Safe Storage password found, if any.
pub fn safe_storage_password() -> Option<String> {
    safe_storage_passwords().into_iter().next()
}

/// Decrypts a token the Electron app stored: the base64 of a `safeStorage` blob.
///
/// `None` for anything that is not a `v10` blob, does not decrypt under this
/// password (bad padding), or does not come out as UTF-8 - a wrong key yields
/// garbage, and garbage is not a token.
pub fn decrypt_safe_storage(blob_base64: &str, password: &str) -> Option<String> {
    let blob = BASE64.decode(blob_base64.trim()).ok()?;
    let ciphertext = blob.strip_prefix(SAFE_STORAGE_PREFIX)?;
    if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
        return None;
    }
    let key = derive_key(password)?;
    let plain = cc_crypt(CC_DECRYPT, &key, ciphertext)?;
    String::from_utf8(plain).ok()
}

/// The inverse of [`decrypt_safe_storage`], for building fixtures in tests.
#[cfg(test)]
pub(crate) fn encrypt_safe_storage(token: &str, password: &str) -> Option<String> {
    let key = derive_key(password)?;
    let mut blob = SAFE_STORAGE_PREFIX.to_vec();
    blob.extend(cc_crypt(CC_ENCRYPT, &key, token.as_bytes())?);
    Some(BASE64.encode(blob))
}

// ---------------------------------------------------------------------------
// CommonCrypto, from libSystem (<CommonCrypto/CommonKeyDerivation.h> and
// <CommonCrypto/CommonCryptor.h>).
// ---------------------------------------------------------------------------

const K_CC_PBKDF2: c_uint = 2;
const K_CC_PRF_HMAC_ALG_SHA1: c_uint = 1;
#[cfg(test)]
const CC_ENCRYPT: u32 = 0;
const CC_DECRYPT: u32 = 1;
const K_CC_ALGORITHM_AES: u32 = 0;
const K_CC_OPTION_PKCS7_PADDING: u32 = 1;
const K_CC_SUCCESS: i32 = 0;

unsafe extern "C" {
    fn CCKeyDerivationPBKDF(
        algorithm: c_uint,
        password: *const u8,
        password_len: usize,
        salt: *const u8,
        salt_len: usize,
        prf: c_uint,
        rounds: c_uint,
        derived_key: *mut u8,
        derived_key_len: usize,
    ) -> c_int;

    fn CCCrypt(
        op: u32,
        alg: u32,
        options: u32,
        key: *const c_void,
        key_length: usize,
        iv: *const c_void,
        data_in: *const c_void,
        data_in_length: usize,
        data_out: *mut c_void,
        data_out_available: usize,
        data_out_moved: *mut usize,
    ) -> i32;
}

/// PBKDF2-HMAC-SHA1 of `password` with Chromium's fixed salt and round count.
fn derive_key(password: &str) -> Option<[u8; SAFE_STORAGE_KEY_LEN]> {
    let mut key = [0u8; SAFE_STORAGE_KEY_LEN];
    // SAFETY: every pointer is paired with the length of the buffer it points into,
    // the buffers outlive the call, and `key` is writable for `key.len()` bytes.
    let status = unsafe {
        CCKeyDerivationPBKDF(
            K_CC_PBKDF2,
            password.as_ptr(),
            password.len(),
            SAFE_STORAGE_SALT.as_ptr(),
            SAFE_STORAGE_SALT.len(),
            K_CC_PRF_HMAC_ALG_SHA1,
            SAFE_STORAGE_ITERATIONS,
            key.as_mut_ptr(),
            key.len(),
        )
    };
    (status == K_CC_SUCCESS).then_some(key)
}

/// One-shot AES-128-CBC with PKCS#7 padding and the sixteen-space IV.
fn cc_crypt(op: u32, key: &[u8; SAFE_STORAGE_KEY_LEN], input: &[u8]) -> Option<Vec<u8>> {
    // Padding adds at most one block; decrypting never grows the data.
    let mut out = vec![0u8; input.len() + 16];
    let mut moved: usize = 0;
    // SAFETY: `key` is 16 bytes (AES-128), the IV is one 16-byte block, `input` is
    // read for its own length, `out` is writable for `out.len()` bytes - enough for
    // the output, as CommonCrypto requires - and `moved` is a valid out-pointer.
    let status = unsafe {
        CCCrypt(
            op,
            K_CC_ALGORITHM_AES,
            K_CC_OPTION_PKCS7_PADDING,
            key.as_ptr().cast(),
            key.len(),
            SAFE_STORAGE_IV.as_ptr().cast(),
            input.as_ptr().cast(),
            input.len(),
            out.as_mut_ptr().cast(),
            out.len(),
            &mut moved,
        )
    };
    if status != K_CC_SUCCESS || moved > out.len() {
        return None;
    }
    out.truncate(moved);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn derives_the_key_chromium_derives() {
        // Computed independently: python3 hashlib.pbkdf2_hmac('sha1', password,
        // b'saltysalt', 1003, 16).
        assert_eq!(
            derive_key("peanuts").map(|key| hex(&key)).as_deref(),
            Some("d9a09d499b4e1b7461f28e67972c6dbd")
        );
        assert_eq!(
            derive_key("Xk3vQ9bL0mZp7Rt2Wy5uHg==")
                .map(|key| hex(&key))
                .as_deref(),
            Some("06ab774fd2f31237b304cbbf71af5278")
        );
    }

    #[test]
    fn decrypts_a_blob_encrypted_independently() {
        // `v10` + `openssl enc -aes-128-cbc -K <key above> -iv 2020...20`, base64.
        assert_eq!(
            decrypt_safe_storage(
                "djEwYAo1slY1oluFXS34vk1PusCzbYGFYTANJzYgfr6IC92FDzOM2iiiRwSwfeD8pVv3",
                "Xk3vQ9bL0mZp7Rt2Wy5uHg=="
            )
            .as_deref(),
            Some("ghp_ExampleToken1234567890abcdefXYZ")
        );
        // Multi-byte UTF-8 survives, and the padding lands mid-character safely.
        assert_eq!(
            decrypt_safe_storage(
                "djEwRGbp6SSYy/oBco4Q8YN+QO7vzbZz0Vi9CibR92GQmgM=",
                "peanuts"
            )
            .as_deref(),
            Some("glpat-é✓ token")
        );
    }

    #[test]
    fn round_trips_through_common_crypto() {
        for token in [
            "",
            "a",
            "exactly sixteen!",
            "ghp_0123456789abcdefghij",
            "žluťoučký kůň",
        ] {
            let blob = encrypt_safe_storage(token, "secret").expect("encrypts");
            assert!(blob.starts_with("djEw"), "a v10 blob in base64");
            assert_eq!(
                decrypt_safe_storage(&blob, "secret").as_deref(),
                Some(token)
            );
        }
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        let blob = encrypt_safe_storage("ghp_token", "right").expect("encrypts");
        // A wrong key: bad padding or garbage, never a token.
        assert_eq!(decrypt_safe_storage(&blob, "wrong"), None);
        // Not base64.
        assert_eq!(decrypt_safe_storage("not base64 at all!", "right"), None);
        // No v10 prefix.
        assert_eq!(
            decrypt_safe_storage(&BASE64.encode(b"v11abcdefghijklmnop"), "right"),
            None
        );
        // Truncated ciphertext.
        let mut raw = BASE64.decode(&blob).expect("base64");
        raw.truncate(raw.len() - 3);
        assert_eq!(decrypt_safe_storage(&BASE64.encode(raw), "right"), None);
        // Just the prefix.
        assert_eq!(decrypt_safe_storage("djEw", "right"), None);
    }

    /// Exercises the real Keychain with a throwaway service and cleans up after
    /// itself. Ignored by default because it may prompt.
    #[test]
    #[ignore]
    fn the_real_keychain_round_trips_a_token() {
        let store = KeychainTokens::with_service("cz.mares.reviewdeck.test-throwaway");
        let account = "test-account";
        store.set(account, "first").expect("set");
        assert_eq!(store.get(account).expect("get").as_deref(), Some("first"));
        store.set(account, "second").expect("overwrite");
        assert_eq!(store.get(account).expect("get").as_deref(), Some("second"));
        store.delete(account).expect("delete");
        assert_eq!(store.get(account).expect("get"), None);
        store.delete(account).expect("deleting nothing is fine");
    }
}
