//! NIP-47 (Nostr Wallet Connect) protocol types

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::fmt;
use std::str::FromStr;
use strum::{Display, EnumString};

use crate::lightning::PaymentResult;
use crate::models::{
    Amount, Bolt11String, Description, Expiry, PaymentHash, PaymentType, Preimage, PublicKey,
    Timestamp, Transaction, TransactionState, TransactionType,
};

/// NWC request methods
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum NwcMethod {
    PayInvoice,
    MultiPayInvoice,
    PayKeysend,
    MultiPayKeysend,
    MakeInvoice,
    LookupInvoice,
    ListTransactions,
    GetBalance,
    GetInfo,
}

/// NWC notification types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum NwcNotificationType {
    PaymentReceived,
    PaymentSent,
}

/// Parsed NWC method - either a known method or an unrecognized string.
///
/// This allows type-safe handling of known methods while gracefully
/// handling unknown methods with proper error responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedMethod {
    /// A recognized NWC method
    Known(NwcMethod),
    /// An unrecognized method string (preserved for error messages)
    Unknown(String),
}

impl ParsedMethod {
    /// Returns the known method, or None if unknown
    pub fn as_known(&self) -> Option<&NwcMethod> {
        match self {
            ParsedMethod::Known(m) => Some(m),
            ParsedMethod::Unknown(_) => None,
        }
    }

    /// Returns the method as a string (for error messages and serialization)
    pub fn as_str(&self) -> &str {
        match self {
            ParsedMethod::Known(m) => m.as_str(),
            ParsedMethod::Unknown(s) => s,
        }
    }
}

impl NwcMethod {
    /// Returns the method as a string
    pub fn as_str(&self) -> &str {
        match self {
            NwcMethod::PayInvoice => "pay_invoice",
            NwcMethod::MultiPayInvoice => "multi_pay_invoice",
            NwcMethod::PayKeysend => "pay_keysend",
            NwcMethod::MultiPayKeysend => "multi_pay_keysend",
            NwcMethod::MakeInvoice => "make_invoice",
            NwcMethod::LookupInvoice => "lookup_invoice",
            NwcMethod::ListTransactions => "list_transactions",
            NwcMethod::GetBalance => "get_balance",
            NwcMethod::GetInfo => "get_info",
        }
    }
}

impl<'de> Deserialize<'de> for ParsedMethod {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match NwcMethod::from_str(&s) {
            Ok(method) => Ok(ParsedMethod::Known(method)),
            Err(_) => Ok(ParsedMethod::Unknown(s)),
        }
    }
}

impl Serialize for ParsedMethod {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl fmt::Display for ParsedMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// NWC request structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NwcRequest {
    pub method: ParsedMethod,
    pub params: Value,
}

/// NWC request with sender context for authorization
/// This wraps the raw NWC request with metadata from the Nostr event
#[derive(Debug, Clone)]
pub struct NwcRequestContext {
    pub request: NwcRequest,
    pub sender_pubkey: PublicKey,
    pub event_id: String,
}

/// NWC response structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NwcResponse {
    pub result_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<NwcError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

/// NWC error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NwcError {
    pub code: NwcErrorCode,
    pub message: String,
}

/// Standard NWC error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NwcErrorCode {
    RateLimited,
    NotImplemented,
    InsufficientBalance,
    QuotaExceeded,
    Restricted,
    Unauthorized,
    Internal,
    PaymentFailed,
    NotFound,
    BadRequest,
    AlreadyPaid,
    PaymentInProgress,
    Other,
}

impl NwcErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RateLimited => "RATE_LIMITED",
            Self::NotImplemented => "NOT_IMPLEMENTED",
            Self::InsufficientBalance => "INSUFFICIENT_BALANCE",
            Self::QuotaExceeded => "QUOTA_EXCEEDED",
            Self::Restricted => "RESTRICTED",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::Internal => "INTERNAL",
            Self::PaymentFailed => "PAYMENT_FAILED",
            Self::NotFound => "NOT_FOUND",
            Self::BadRequest => "BAD_REQUEST",
            Self::AlreadyPaid => "ALREADY_PAID",
            Self::PaymentInProgress => "PAYMENT_IN_PROGRESS",
            Self::Other => "OTHER",
        }
    }
}

// Result types for NWC protocol responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayInvoiceResult {
    pub preimage: Preimage,
    pub fees_paid: Option<Amount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBalanceResult {
    pub balance: Amount,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakeInvoiceResult {
    #[serde(rename = "type")]
    pub invoice_type: String,
    pub state: TransactionState,
    pub invoice: Bolt11String,
    pub description: Option<Description>,
    pub payment_hash: PaymentHash,
    pub amount: Option<Amount>,
    pub created_at: Timestamp,
    pub expires_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionInfo {
    #[serde(rename = "type")]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListTransactionsResult {
    pub transactions: Vec<TransactionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInfoResult {
    pub alias: String,
    pub color: String,
    pub pubkey: PublicKey,
    pub network: String,
    pub block_height: u64,
    pub block_hash: String,
    pub methods: Vec<String>,
    pub notifications: Vec<String>,
}

impl NwcResponse {
    /// Create a successful pay_invoice response
    pub fn pay_invoice(result: PaymentResult) -> Self {
        let pay_result = PayInvoiceResult {
            preimage: result.preimage,
            fees_paid: result.fees_paid,
        };
        match serde_json::to_value(pay_result) {
            Ok(value) => Self {
                result_type: "pay_invoice".to_string(),
                error: None,
                result: Some(value),
            },
            Err(error) => Self::error(
                "pay_invoice".to_string(),
                NwcErrorCode::Internal,
                format!("Failed to serialize response: {error}"),
            ),
        }
    }

    /// Create a successful get_balance response
    pub fn get_balance(balance_msats: u64) -> Self {
        let balance_result = GetBalanceResult {
            balance: Amount::from_msats(balance_msats),
        };
        match serde_json::to_value(balance_result) {
            Ok(value) => Self {
                result_type: "get_balance".to_string(),
                error: None,
                result: Some(value),
            },
            Err(error) => Self::error(
                "get_balance".to_string(),
                NwcErrorCode::Internal,
                format!("Failed to serialize response: {error}"),
            ),
        }
    }

    /// Create a successful make_invoice response
    pub fn make_invoice(invoice: crate::models::Invoice, transaction: Transaction) -> Self {
        let make_result = MakeInvoiceResult {
            invoice_type: "incoming".to_string(),
            state: transaction.state,
            invoice: invoice.bolt11,
            description: invoice.description,
            payment_hash: invoice.payment_hash,
            amount: invoice.amount,
            created_at: transaction.created_at,
            expires_at: invoice.expiry.map(|e| transaction.created_at + e.as_secs()),
        };
        match serde_json::to_value(make_result) {
            Ok(value) => Self {
                result_type: "make_invoice".to_string(),
                error: None,
                result: Some(value),
            },
            Err(error) => Self::error(
                "make_invoice".to_string(),
                NwcErrorCode::Internal,
                format!("Failed to serialize response: {error}"),
            ),
        }
    }

    /// Create a successful list_transactions response
    pub fn list_transactions(transactions: Vec<Transaction>) -> Self {
        let tx_list: Vec<TransactionInfo> = transactions
            .into_iter()
            .map(|tx| TransactionInfo {
                transaction_type: tx.transaction_type,
                state: tx.state,
                invoice: tx.invoice,
                description: tx.description,
                preimage: tx.preimage,
                payment_hash: tx.payment_hash,
                amount: tx.amount,
                fees_paid: tx.fees_paid,
                created_at: tx.created_at,
                settled_at: tx.settled_at,
                metadata: tx.metadata,
            })
            .collect();

        let list_result = ListTransactionsResult {
            transactions: tx_list,
        };

        match serde_json::to_value(list_result) {
            Ok(value) => Self {
                result_type: "list_transactions".to_string(),
                error: None,
                result: Some(value),
            },
            Err(error) => Self::error(
                "list_transactions".to_string(),
                NwcErrorCode::Internal,
                format!("Failed to serialize response: {error}"),
            ),
        }
    }

    /// Create a successful get_info response
    pub fn get_info(
        pubkey: String,
        network: String,
        block_height: u64,
        block_hash: Option<String>,
        methods: Vec<String>,
        notifications: Vec<String>,
    ) -> Self {
        // Use provided block_hash or default placeholder
        // TODO: Fedimint wallet module should provide the actual block hash
        let block_hash = block_hash.unwrap_or_else(|| {
            "0000000000000000000000000000000000000000000000000000000000000000".to_string()
        });

        let info_result = GetInfoResult {
            alias: "Nanduti".to_string(),
            color: "#FF6B00".to_string(),
            pubkey: PublicKey::new(pubkey),
            network,
            block_height,
            block_hash,
            methods,
            notifications,
        };
        match serde_json::to_value(info_result) {
            Ok(value) => Self {
                result_type: "get_info".to_string(),
                error: None,
                result: Some(value),
            },
            Err(error) => Self::error(
                "get_info".to_string(),
                NwcErrorCode::Internal,
                format!("Failed to serialize response: {error}"),
            ),
        }
    }

    /// Create a successful lookup_invoice response
    pub fn lookup_invoice(result: Value) -> Self {
        Self {
            result_type: "lookup_invoice".to_string(),
            error: None,
            result: Some(result),
        }
    }

    /// Create an error response
    pub fn error(result_type: String, code: NwcErrorCode, message: String) -> Self {
        Self {
            result_type,
            error: Some(NwcError { code, message }),
            result: None,
        }
    }
}

/// NWC pay_invoice parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayInvoiceParams {
    pub invoice: Bolt11String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<Amount>,
}

/// NWC make_invoice parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakeInvoiceParams {
    pub amount: Amount,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_hash: Option<String>, // Keep as String for hash
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiry: Option<Expiry>,
}

/// NWC list_transactions parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListTransactionsParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unpaid: Option<bool>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub transaction_type: Option<TransactionType>,
}

/// NWC pay_keysend parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayKeysendParams {
    pub amount: Amount,
    pub pubkey: PublicKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preimage: Option<Preimage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tlv_records: Option<Vec<TlvRecord>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlvRecord {
    #[serde(rename = "type")]
    pub tlv_type: u64,
    pub value: String, // hex encoded
}

// Notification types for Nostr client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NostrNotification {
    pub notification_type: NwcNotificationType,
    pub notification: NotificationData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NotificationData {
    PaymentReceived(PaymentReceivedNotification),
    PaymentSent(PaymentSentNotification),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentReceivedNotification {
    #[serde(rename = "type")]
    pub payment_type: PaymentType,
    pub state: TransactionState,
    pub invoice: Bolt11String,
    pub payment_hash: PaymentHash,
    pub preimage: Preimage,
    pub amount: Amount,
    pub settled_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentSentNotification {
    #[serde(rename = "type")]
    pub payment_type: PaymentType,
    pub state: TransactionState,
    pub invoice: Bolt11String,
    pub payment_hash: PaymentHash,
    pub preimage: Preimage,
    pub amount: Amount,
    pub fees_paid: Option<Amount>,
    pub settled_at: Timestamp,
}
