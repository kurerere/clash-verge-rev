use aes::cipher::{BlockModeDecrypt as _, KeyIvInit as _, block_padding::Pkcs7};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use md5::{Digest as _, Md5};

pub const ENCRYPTION_HEADER: &str = "subscription-encryption";

pub fn is_encrypted(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("true")
}

pub fn decrypt(password: &str, body: &str) -> Option<String> {
    let encoded = body.replace(['\r', '\n'], "");
    let decoded = STANDARD.decode(encoded).ok()?;
    if decoded.len() <= 16 {
        return None;
    }
    let (iv, ciphertext) = decoded.split_at(16);
    let key = Md5::digest(password.as_bytes());
    let plaintext = cbc::Decryptor::<aes::Aes128>::new_from_slices(&key, iv)
        .ok()?
        .decrypt_padded_vec::<Pkcs7>(ciphertext)
        .ok()?;
    String::from_utf8(plaintext).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionPasswordError {
    Required,
    Incorrect,
}

impl SubscriptionPasswordError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Required => "PROFILE_SUBSCRIPTION_PASSWORD_REQUIRED",
            Self::Incorrect => "PROFILE_SUBSCRIPTION_PASSWORD_INCORRECT",
        }
    }

    pub fn from_chain(err: &anyhow::Error) -> Option<Self> {
        err.chain().find_map(|cause| cause.downcast_ref::<Self>().copied())
    }
}

impl std::fmt::Display for SubscriptionPasswordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Required => "subscription is encrypted; import it from the Profiles page and enter its password",
            Self::Incorrect => "incorrect subscription password",
        })
    }
}

impl std::error::Error for SubscriptionPasswordError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypts_v2rayn_subscription() {
        let body = "AAECAwQFBgcICQoLDA0OD1pJgyEobGAKlwC5Mxe2ohzR6EyBFvV5UPh6H0WwQHSmqgNeC3xo/avhJx3a+bMoOw==";
        assert_eq!(
            decrypt("verge-test-pass", body).as_deref(),
            Some("proxies:\n  - {name: a, type: direct}\n")
        );
        assert_eq!(decrypt("wrong", body), None);
        assert!(is_encrypted(" TRUE "));
        assert!(!is_encrypted("false"));
    }
}
