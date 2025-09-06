//! Data models for nanduti

use anyhow::Result;
use fedimint_core::Amount as FedimintAmount;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use strum::{Display, EnumString};

/// Amount wrapper around fedimint_core::Amount
/// This provides compatibility while using Fedimint's robust parsing
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Amount(pub FedimintAmount);

impl Amount {
    pub fn from_sats(sats: u64) -> Self {
        Amount(FedimintAmount::from_sats(sats))
    }

    pub fn from_msats(msats: u64) -> Self {
        Amount(FedimintAmount::from_msats(msats))
    }

    pub fn as_sats(&self) -> u64 {
        self.0.sats_round_down()
    }

    pub fn as_msats(&self) -> u64 {
        self.0.msats
    }
}

impl std::fmt::Display for Amount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Amount {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(Amount(FedimintAmount::from_str(s)?))
    }
}

impl From<FedimintAmount> for Amount {
    fn from(amt: FedimintAmount) -> Self {
        Amount(amt)
    }
}

impl From<Amount> for FedimintAmount {
    fn from(amt: Amount) -> Self {
        amt.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_amount_parsing() -> Result<()> {
        // Test satoshi formats
        assert_eq!(Amount::from_str("100sat")?.as_sats(), 100);
        assert_eq!(Amount::from_str("100 sat")?.as_sats(), 100);

        // Test millisatoshi formats
        assert_eq!(Amount::from_str("1000msat")?.as_msats(), 1000);
        assert_eq!(Amount::from_str("1000 msat")?.as_msats(), 1000);

        // Test bitcoin formats
        assert_eq!(Amount::from_str("0.001btc")?.as_sats(), 100_000);
        assert_eq!(Amount::from_str("0.00000001btc")?.as_sats(), 1);
        assert_eq!(Amount::from_str("1btc")?.as_sats(), 100_000_000);
        assert_eq!(Amount::from_str("0.00001 btc")?.as_sats(), 1000);

        // Test plain numbers (defaults to millisats in Fedimint)
        assert_eq!(Amount::from_str("42")?.as_msats(), 42);
        assert_eq!(Amount::from_str("1000")?.as_msats(), 1000);

        // Test error cases
        assert!(Amount::from_str("").is_err());
        assert!(Amount::from_str("notanumber").is_err());

        Ok(())
    }
}

/// Lightning invoice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    pub bolt11: Bolt11String,
    pub payment_hash: PaymentHash,
    pub amount: Option<Amount>,
    pub description: Option<Description>,
    pub expiry: Option<Expiry>,
    pub payee_pubkey: Option<PublicKey>,
}

impl From<&lightning_invoice::Bolt11Invoice> for Invoice {
    fn from(bolt11: &lightning_invoice::Bolt11Invoice) -> Self {
        use hex;

        Invoice {
            bolt11: Bolt11String(bolt11.to_string()),
            payment_hash: PaymentHash(hex::encode(bolt11.payment_hash().as_ref() as &[u8])),
            amount: bolt11.amount_milli_satoshis().map(Amount::from_msats),
            description: match bolt11.description() {
                lightning_invoice::Bolt11InvoiceDescriptionRef::Direct(desc) => {
                    Some(Description(desc.to_string()))
                }
                lightning_invoice::Bolt11InvoiceDescriptionRef::Hash(_) => None,
            },
            expiry: Some(Expiry(bolt11.expiry_time().as_secs())),
            payee_pubkey: bolt11.payee_pub_key().map(|k| PublicKey(k.to_string())),
        }
    }
}

impl TryFrom<&Invoice> for lightning_invoice::Bolt11Invoice {
    type Error = anyhow::Error;

    fn try_from(invoice: &Invoice) -> Result<Self> {
        use std::str::FromStr;
        lightning_invoice::Bolt11Invoice::from_str(invoice.bolt11.as_str())
            .map_err(|e| anyhow::anyhow!("Failed to parse BOLT11 invoice: {e}"))
    }
}

/// Transaction record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: TransactionId,
    pub federation_id: FederationId,
    pub transaction_type: TransactionType,
    pub state: TransactionState,
    pub invoice: Option<Bolt11String>,
    pub description: Option<Description>,
    pub preimage: Option<Preimage>,
    pub payment_hash: PaymentHash,
    pub amount: Amount,
    pub fees_paid: Option<Amount>,
    pub created_at: Timestamp,
    pub settled_at: Option<Timestamp>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum TransactionType {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
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
    pub last_updated: Timestamp,
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

// ============================================================================
// Strong Type Wrappers for Domain Safety
// ============================================================================

/// Payment hash identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaymentHash(pub String);

impl PaymentHash {
    pub fn new(hash: String) -> Self {
        Self(hash)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for PaymentHash {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PaymentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Payment preimage
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Preimage(pub String);

impl Preimage {
    pub fn new(preimage: String) -> Self {
        Self(preimage)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for Preimage {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Preimage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Public key
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PublicKey(pub String);

impl PublicKey {
    pub fn new(key: String) -> Self {
        Self(key)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for PublicKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// BOLT11 invoice string
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bolt11String(pub String);

impl Bolt11String {
    pub fn new(invoice: String) -> Self {
        Self(invoice)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Bolt11String {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Payment description
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Description(pub String);

impl Description {
    pub fn new(desc: String) -> Self {
        Self(desc)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for Description {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Description {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Transaction identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransactionId(pub String);

impl TransactionId {
    pub fn new(id: String) -> Self {
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for TransactionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TransactionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unix timestamp
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub u64);

impl std::ops::Add<u64> for Timestamp {
    type Output = Timestamp;

    fn add(self, rhs: u64) -> Self::Output {
        Timestamp(self.0 + rhs)
    }
}

impl Timestamp {
    pub fn now() -> Self {
        Self(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
    }

    pub fn from_secs(secs: u64) -> Self {
        Self(secs)
    }

    pub fn as_secs(&self) -> u64 {
        self.0
    }

    pub fn as_i64(&self) -> i64 {
        self.0 as i64
    }
}

impl From<u64> for Timestamp {
    fn from(secs: u64) -> Self {
        Self(secs)
    }
}

impl From<Timestamp> for u64 {
    fn from(ts: Timestamp) -> u64 {
        ts.0
    }
}

/// Expiry duration in seconds
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Expiry(pub u64);

impl Expiry {
    pub fn from_secs(secs: u64) -> Self {
        Self(secs)
    }

    pub fn as_secs(&self) -> u64 {
        self.0
    }
}

impl From<u64> for Expiry {
    fn from(secs: u64) -> Self {
        Self(secs)
    }
}

impl From<Expiry> for u64 {
    fn from(exp: Expiry) -> u64 {
        exp.0
    }
}

/// Federation identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FederationId(pub String);

impl FederationId {
    pub fn new(id: String) -> Self {
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for FederationId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for FederationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Federation name
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FederationName(pub String);

impl FederationName {
    pub fn new(name: String) -> Self {
        Self(name)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for FederationName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for FederationName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Gateway identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GatewayId(pub String);

impl GatewayId {
    pub fn new(id: String) -> Self {
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for GatewayId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for GatewayId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// NWC Connection identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectionId(pub String);

impl ConnectionId {
    pub fn new(id: String) -> Self {
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for ConnectionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Connection name
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectionName(pub String);

impl ConnectionName {
    pub fn new(name: String) -> Self {
        Self(name)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for ConnectionName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConnectionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lightning address (LUD16)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LightningAddress(pub String);

impl LightningAddress {
    pub fn new(address: String) -> Self {
        Self(address)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for LightningAddress {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LightningAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Nostr relay URL
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelayUrl(pub String);

impl RelayUrl {
    pub fn new(url: String) -> Self {
        Self(url)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for RelayUrl {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RelayUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Gateway API URL
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GatewayApiUrl(pub String);

impl GatewayApiUrl {
    pub fn new(url: String) -> Self {
        Self(url)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for GatewayApiUrl {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for GatewayApiUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Connection URI for NWC
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectionUri(pub String);

impl ConnectionUri {
    pub fn new(uri: String) -> Self {
        Self(uri)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for ConnectionUri {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConnectionUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
