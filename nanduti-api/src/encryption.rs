//! NIP-44 encryption for NWC messages

use anyhow::{Context, Result};
use nostr::nips::nip44;
use nostr::prelude::*;

/// Encrypt using NIP-44 with Keys
pub fn encrypt_nip44(
    content: &str,
    recipient_pubkey: &PublicKey,
    sender_keys: &Keys,
) -> Result<String> {
    nip44::encrypt(
        sender_keys.secret_key(),
        recipient_pubkey,
        content,
        nip44::Version::V2,
    )
    .context("Failed to encrypt with NIP-44")
}

/// Decrypt using NIP-44 with Keys
pub fn decrypt_nip44(
    encrypted: &str,
    sender_pubkey: &PublicKey,
    recipient_keys: &Keys,
) -> Result<String> {
    nip44::decrypt(recipient_keys.secret_key(), sender_pubkey, encrypted)
        .context("Failed to decrypt with NIP-44")
}
