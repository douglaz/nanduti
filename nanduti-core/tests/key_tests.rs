//! Integration tests for NWC key generation and URI parsing

use anyhow::Result;
use nanduti_core::keys::{NwcConnection, NwcKeys};

#[test]
fn test_key_generation() -> Result<()> {
    // Test key generation
    let keys = NwcKeys::generate()?;
    assert_eq!(keys.secret_key.len(), 64); // 32 bytes hex
    assert_eq!(keys.public_key.len(), 64); // 32 bytes hex

    // Test connection URI generation
    let connection = NwcConnection::generate(
        keys.public_key.clone(),
        vec!["wss://relay.damus.io".to_string()],
        Some("alice@example.com".to_string()),
    )?;

    let uri = connection.to_uri();
    assert!(uri.starts_with("nostr+walletconnect://"));
    assert!(uri.contains("relay="));
    assert!(uri.contains("secret="));
    assert!(uri.contains("lud16="));

    // Test URI parsing
    let parsed = NwcConnection::from_uri(&uri)?;
    assert_eq!(parsed.wallet_pubkey, connection.wallet_pubkey);
    assert_eq!(parsed.relays, connection.relays);
    assert_eq!(parsed.secret, connection.secret);
    assert_eq!(parsed.lud16, connection.lud16);

    Ok(())
}

#[test]
fn test_parse_nip47_uri() -> Result<()> {
    // Test with example from NIP-47 spec
    let uri = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";

    let connection = NwcConnection::from_uri(uri)?;

    assert_eq!(
        connection.wallet_pubkey,
        "b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4"
    );
    assert_eq!(connection.relays, vec!["wss://relay.damus.io"]);
    assert_eq!(
        connection.secret,
        "71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c"
    );
    assert_eq!(connection.lud16, None);

    Ok(())
}
