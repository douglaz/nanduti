//! Integration tests for NIP-44 encryption

use anyhow::Result;

#[tokio::test]
async fn test_encryption() -> Result<()> {
    use nanduti_api::encryption;
    use nostr_sdk::prelude::*;

    // Create two key pairs
    let alice_keys = Keys::generate();
    let bob_keys = Keys::generate();

    let message = "Hello, NWC!";

    // Test NIP-44 encryption
    let encrypted_nip44 = encryption::encrypt_nip44(message, &bob_keys.public_key(), &alice_keys)?;

    let decrypted_nip44 =
        encryption::decrypt_nip44(&encrypted_nip44, &alice_keys.public_key(), &bob_keys)?;

    assert_eq!(message, decrypted_nip44);

    Ok(())
}
