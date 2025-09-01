//! Data models for nanduti

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Amount in millisatoshis
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Amount(pub u64);

impl Amount {
    pub fn from_sats(sats: u64) -> Self {
        Amount(sats * 1000)
    }

    pub fn from_msats(msats: u64) -> Self {
        Amount(msats)
    }

    pub fn as_sats(&self) -> u64 {
        self.0 / 1000
    }

    pub fn as_msats(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for Amount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} msats", self.0)
    }
}

impl FromStr for Amount {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        // Remove whitespace
        let s = s.trim();

        // Try to parse different formats
        if s.is_empty() {
            bail!("Empty amount string");
        }

        // Check for unit suffixes (case-insensitive)
        let lower = s.to_lowercase();

        if let Some(btc_str) = lower.strip_suffix("btc") {
            // Bitcoin format (e.g., "0.001btc")
            let btc: f64 = btc_str
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid BTC amount: {btc_str}"))?;
            if btc < 0.0 {
                bail!("Negative amounts not allowed");
            }
            let sats = (btc * 100_000_000.0) as u64;
            Ok(Amount::from_sats(sats))
        } else if let Some(msats_str) = lower
            .strip_suffix("msats")
            .or_else(|| lower.strip_suffix("msat"))
        {
            // Millisatoshi format (e.g., "1000msats" or "1000msat")
            let msats: u64 = msats_str
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid msats amount: {msats_str}"))?;
            Ok(Amount::from_msats(msats))
        } else if let Some(sats_str) = lower
            .strip_suffix("sats")
            .or_else(|| lower.strip_suffix("sat"))
        {
            // Satoshi format (e.g., "100sats" or "100sat")
            let sats: u64 = sats_str
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid sats amount: {sats_str}"))?;
            Ok(Amount::from_sats(sats))
        } else {
            // Try to parse as plain number (assume sats for backward compatibility)
            match s.parse::<u64>() {
                Ok(sats) => Ok(Amount::from_sats(sats)),
                Err(_) => {
                    // Try to parse as float (assume BTC)
                    match s.parse::<f64>() {
                        Ok(btc) if btc >= 0.0 => {
                            let sats = (btc * 100_000_000.0) as u64;
                            Ok(Amount::from_sats(sats))
                        }
                        _ => bail!("Invalid amount format: {s}. Use formats like '100sats', '0.001btc', or '1000msats'")
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_amount_parsing() -> Result<()> {
        // Test satoshi formats
        assert_eq!(Amount::from_str("100sats")?.as_sats(), 100);
        assert_eq!(Amount::from_str("100sat")?.as_sats(), 100);
        assert_eq!(Amount::from_str("100SATS")?.as_sats(), 100);
        assert_eq!(Amount::from_str(" 100 sats ")?.as_sats(), 100);

        // Test millisatoshi formats
        assert_eq!(Amount::from_str("1000msats")?.as_msats(), 1000);
        assert_eq!(Amount::from_str("1000msat")?.as_msats(), 1000);
        assert_eq!(Amount::from_str("2500MSATS")?.as_msats(), 2500);

        // Test bitcoin formats
        assert_eq!(Amount::from_str("0.001btc")?.as_sats(), 100_000);
        assert_eq!(Amount::from_str("0.00000001btc")?.as_sats(), 1);
        assert_eq!(Amount::from_str("1BTC")?.as_sats(), 100_000_000);
        assert_eq!(Amount::from_str("0.00001btc")?.as_sats(), 1000);

        // Test plain numbers (defaults to sats)
        assert_eq!(Amount::from_str("42")?.as_sats(), 42);
        assert_eq!(Amount::from_str("1000")?.as_sats(), 1000);

        // Test float numbers (assumes BTC)
        assert_eq!(Amount::from_str("0.001")?.as_sats(), 100_000);
        assert_eq!(Amount::from_str("1.5")?.as_sats(), 150_000_000);

        // Test error cases
        assert!(Amount::from_str("").is_err());
        assert!(Amount::from_str("-100sats").is_err());
        assert!(Amount::from_str("-0.001btc").is_err());
        assert!(Amount::from_str("notanumber").is_err());
        assert!(Amount::from_str("100xyz").is_err());

        Ok(())
    }
}

/// Lightning invoice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    pub bolt11: String,
    pub payment_hash: String,
    pub amount: Option<Amount>,
    pub description: Option<String>,
    pub expiry: Option<u64>,
    pub payee_pubkey: Option<String>,
}

/// Transaction record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: String,
    pub federation_id: String,
    pub transaction_type: TransactionType,
    pub state: TransactionState,
    pub invoice: Option<String>,
    pub description: Option<String>,
    pub preimage: Option<String>,
    pub payment_hash: String,
    pub amount: Amount,
    pub fees_paid: Option<Amount>,
    pub created_at: u64,
    pub settled_at: Option<u64>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionType {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionState {
    Pending,
    Settled,
    Failed,
    Expired,
}

/// Federation metrics for routing decisions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationMetrics {
    pub uptime_percent: f64,
    pub success_rate: f64,
    pub average_fee: Amount,
    pub average_latency_ms: u64,
    pub total_payments: u64,
    pub total_volume: Amount,
    pub last_updated: u64,
}

/// Gateway vetting status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayVettingStatus {
    /// Gateway is explicitly approved by the federation
    Vetted,
    /// Gateway is not approved (vetted list exists but gateway is not in it)
    NotVetted,
    /// No vetting policy exists (all gateways are acceptable)
    Unknown,
}
