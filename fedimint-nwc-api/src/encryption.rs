//! NIP-44 and NIP-04 encryption for NWC messages

use anyhow::{Context, Result};
use nostr::nips::nip44;
use nostr::prelude::*;

/// Encryption methods supported by NWC
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionMethod {
    Nip44,
    Nip04, // Legacy, for backwards compatibility
}

/// Encrypt a message using the specified method
pub fn encrypt_message(
    content: &str,
    sender_secret: &SecretKey,
    recipient_pubkey: &PublicKey,
    method: EncryptionMethod,
) -> Result<String> {
    match method {
        EncryptionMethod::Nip44 => {
            nip44::encrypt(sender_secret, recipient_pubkey, content, nip44::Version::V2)
                .context("Failed to encrypt with NIP-44")
        }
        EncryptionMethod::Nip04 => {
            nostr::nips::nip04::encrypt(sender_secret, recipient_pubkey, content)
                .context("Failed to encrypt with NIP-04")
        }
    }
}

/// Decrypt a message using the specified method
pub fn decrypt_message(
    encrypted: &str,
    recipient_secret: &SecretKey,
    sender_pubkey: &PublicKey,
    method: EncryptionMethod,
) -> Result<String> {
    match method {
        EncryptionMethod::Nip44 => nip44::decrypt(recipient_secret, sender_pubkey, encrypted)
            .context("Failed to decrypt with NIP-44"),
        EncryptionMethod::Nip04 => {
            nostr::nips::nip04::decrypt(recipient_secret, sender_pubkey, encrypted)
                .context("Failed to decrypt with NIP-04")
        }
    }
}

/// Parse encryption method from event tags
pub fn parse_encryption_method(tags: &[Tag]) -> EncryptionMethod {
    // Look for encryption tag
    for tag in tags {
        if tag.kind() == TagKind::Custom("encryption".into()) {
            // Get the tag content (first value after the tag name)
            if let Some(method) = tag.content() {
                if method.contains("nip44") {
                    return EncryptionMethod::Nip44;
                }
            }
        }
    }
    // Default to NIP-04 for backwards compatibility
    EncryptionMethod::Nip04
}

/// Helper functions for working with Keys

/// Encrypt using NIP-44 with Keys
pub fn encrypt_nip44(
    content: &str,
    recipient_pubkey: &PublicKey,
    sender_keys: &Keys,
) -> Result<String> {
    encrypt_message(
        content,
        sender_keys.secret_key(),
        recipient_pubkey,
        EncryptionMethod::Nip44,
    )
}

/// Decrypt using NIP-44 with Keys
pub fn decrypt_nip44(
    encrypted: &str,
    sender_pubkey: &PublicKey,
    recipient_keys: &Keys,
) -> Result<String> {
    decrypt_message(
        encrypted,
        recipient_keys.secret_key(),
        sender_pubkey,
        EncryptionMethod::Nip44,
    )
}

/// Encrypt using NIP-04 with Keys
pub fn encrypt_nip04(
    content: &str,
    recipient_pubkey: &PublicKey,
    sender_keys: &Keys,
) -> Result<String> {
    encrypt_message(
        content,
        sender_keys.secret_key(),
        recipient_pubkey,
        EncryptionMethod::Nip04,
    )
}

/// Decrypt using NIP-04 with Keys
pub fn decrypt_nip04(
    encrypted: &str,
    sender_pubkey: &PublicKey,
    recipient_keys: &Keys,
) -> Result<String> {
    decrypt_message(
        encrypted,
        recipient_keys.secret_key(),
        sender_pubkey,
        EncryptionMethod::Nip04,
    )
}
