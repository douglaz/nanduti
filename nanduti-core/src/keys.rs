//! Key generation and management for NWC connections

use anyhow::{Context, Result};
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::fmt;

/// NWC connection keys
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NwcKeys {
    /// Secret key for this connection
    pub secret_key: String,
    /// Public key derived from secret
    pub public_key: String,
    /// Nostr Keys object for operations
    #[serde(skip)]
    pub keys: Option<Keys>,
}

impl NwcKeys {
    /// Generate new random keys for a connection
    pub fn generate() -> Result<Self> {
        let keys = Keys::generate();
        let secret_key = keys.secret_key().to_secret_hex();
        let public_key = keys.public_key().to_hex();

        Ok(Self {
            secret_key: secret_key.clone(),
            public_key,
            keys: Some(keys),
        })
    }

    /// Create from existing secret key
    pub fn from_secret(secret_key: &str) -> Result<Self> {
        let keys = Keys::parse(secret_key).context("Failed to parse secret key")?;
        let public_key = keys.public_key().to_hex();

        Ok(Self {
            secret_key: secret_key.to_string(),
            public_key,
            keys: Some(keys),
        })
    }

    /// Get the Nostr Keys object, initializing if needed
    pub fn get_keys(&mut self) -> Result<&Keys> {
        if self.keys.is_none() {
            self.keys = Some(Keys::parse(&self.secret_key)?);
        }
        Ok(self.keys.as_ref().unwrap())
    }
}

/// NWC connection URI components
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NwcConnection {
    /// Wallet service public key
    pub wallet_pubkey: String,
    /// Relays to use for communication
    pub relays: Vec<String>,
    /// Client secret key (32 bytes hex)
    pub secret: String,
    /// Optional lightning address
    pub lud16: Option<String>,
}

impl NwcConnection {
    /// Generate a new connection URI
    pub fn generate(
        wallet_pubkey: String,
        relays: Vec<String>,
        lud16: Option<String>,
    ) -> Result<Self> {
        // Generate random 32-byte secret
        let secret_key = Keys::generate();
        let secret = secret_key.secret_key().to_secret_hex();

        Ok(Self {
            wallet_pubkey,
            relays,
            secret,
            lud16,
        })
    }

    /// Build the connection URI string
    pub fn to_uri(&self) -> String {
        let mut params = vec![];

        // Add relays
        for relay in &self.relays {
            let encoded_relay = urlencoding::encode(relay);
            params.push(format!("relay={encoded_relay}"));
        }

        // Add secret
        let secret = &self.secret;
        params.push(format!("secret={secret}"));

        // Add optional lud16
        if let Some(lud16) = &self.lud16 {
            let encoded_lud16 = urlencoding::encode(lud16);
            params.push(format!("lud16={encoded_lud16}"));
        }

        let wallet_pubkey = &self.wallet_pubkey;
        let query_string = params.join("&");
        format!("nostr+walletconnect://{wallet_pubkey}?{query_string}")
    }

    /// Parse a connection URI
    pub fn from_uri(uri: &str) -> Result<Self> {
        // Parse the URI
        let uri = uri
            .strip_prefix("nostr+walletconnect://")
            .context("Invalid NWC URI prefix")?;

        let parts: Vec<&str> = uri.splitn(2, '?').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid NWC URI format");
        }

        let wallet_pubkey = parts[0].to_string();
        let query_string = parts[1];

        // Parse query parameters
        let mut relays = Vec::new();
        let mut secret = None;
        let mut lud16 = None;

        for pair in query_string.split('&') {
            let kv: Vec<&str> = pair.splitn(2, '=').collect();
            if kv.len() != 2 {
                continue;
            }

            let key = kv[0];
            let value = urlencoding::decode(kv[1])?.into_owned();

            match key {
                "relay" => relays.push(value),
                "secret" => secret = Some(value),
                "lud16" => lud16 = Some(value),
                _ => {} // Ignore unknown parameters
            }
        }

        let secret = secret.context("Missing secret in NWC URI")?;

        if relays.is_empty() {
            anyhow::bail!("No relays specified in NWC URI");
        }

        Ok(Self {
            wallet_pubkey,
            relays,
            secret,
            lud16,
        })
    }

    /// Get the client keys from the connection secret
    pub fn get_client_keys(&self) -> Result<NwcKeys> {
        NwcKeys::from_secret(&self.secret)
    }
}

impl fmt::Display for NwcConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_uri())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_generation() -> Result<()> {
        let keys = NwcKeys::generate()?;
        assert_eq!(keys.secret_key.len(), 64); // 32 bytes hex
        assert_eq!(keys.public_key.len(), 64); // 32 bytes hex
        assert!(keys.keys.is_some());
        Ok(())
    }

    #[test]
    fn test_connection_uri() -> Result<()> {
        let conn = NwcConnection::generate(
            "02abc123".to_string(),
            vec!["wss://relay.damus.io".to_string()],
            Some("user@example.com".to_string()),
        )?;

        let uri = conn.to_uri();
        assert!(uri.starts_with("nostr+walletconnect://"));
        assert!(uri.contains("relay=wss"));
        assert!(uri.contains("secret="));
        assert!(uri.contains("lud16="));

        // Parse it back
        let parsed = NwcConnection::from_uri(&uri)?;
        assert_eq!(parsed.wallet_pubkey, conn.wallet_pubkey);
        assert_eq!(parsed.relays, conn.relays);
        assert_eq!(parsed.secret, conn.secret);
        assert_eq!(parsed.lud16, conn.lud16);

        Ok(())
    }

    #[test]
    fn test_parse_example_uri() -> Result<()> {
        let uri = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";

        let conn = NwcConnection::from_uri(uri)?;
        assert_eq!(
            conn.wallet_pubkey,
            "b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4"
        );
        assert_eq!(conn.relays, vec!["wss://relay.damus.io"]);
        assert_eq!(
            conn.secret,
            "71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c"
        );
        assert_eq!(conn.lud16, None);

        Ok(())
    }
}
