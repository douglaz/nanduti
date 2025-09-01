//! NIP-47 (Nostr Wallet Connect) protocol types

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::lightning::PaymentResult;
use crate::models::Transaction;

/// NWC request methods
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// NWC request structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NwcRequest {
    pub method: String,
    pub params: Value,
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
    pub code: String,
    pub message: String,
}

/// Standard NWC error codes
#[derive(Debug, Clone)]
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
            Self::Other => "OTHER",
        }
    }
}

impl NwcResponse {
    /// Create a successful pay_invoice response
    pub fn pay_invoice(result: PaymentResult) -> Self {
        Self {
            result_type: "pay_invoice".to_string(),
            error: None,
            result: Some(serde_json::json!({
                "preimage": result.preimage,
                "fees_paid": result.fees_paid.map(|a| a.as_msats()),
            })),
        }
    }

    /// Create a successful get_balance response
    pub fn get_balance(balance_msats: u64) -> Self {
        Self {
            result_type: "get_balance".to_string(),
            error: None,
            result: Some(serde_json::json!({
                "balance": balance_msats,
            })),
        }
    }

    /// Create a successful make_invoice response
    pub fn make_invoice(invoice: crate::models::Invoice, transaction: Transaction) -> Self {
        Self {
            result_type: "make_invoice".to_string(),
            error: None,
            result: Some(serde_json::json!({
                "type": "incoming",
                "state": transaction.state,
                "invoice": invoice.bolt11,
                "description": invoice.description,
                "payment_hash": invoice.payment_hash,
                "amount": invoice.amount.map(|a| a.as_msats()),
                "created_at": transaction.created_at,
                "expires_at": invoice.expiry.map(|e| transaction.created_at + e),
            })),
        }
    }

    /// Create a successful list_transactions response
    pub fn list_transactions(transactions: Vec<Transaction>) -> Self {
        let tx_list: Vec<Value> = transactions
            .into_iter()
            .map(|tx| {
                serde_json::json!({
                    "type": tx.transaction_type,
                    "state": tx.state,
                    "invoice": tx.invoice,
                    "description": tx.description,
                    "preimage": tx.preimage,
                    "payment_hash": tx.payment_hash,
                    "amount": tx.amount.as_msats(),
                    "fees_paid": tx.fees_paid.map(|a| a.as_msats()),
                    "created_at": tx.created_at,
                    "settled_at": tx.settled_at,
                    "metadata": tx.metadata,
                })
            })
            .collect();

        Self {
            result_type: "list_transactions".to_string(),
            error: None,
            result: Some(serde_json::json!({
                "transactions": tx_list,
            })),
        }
    }

    /// Create a successful get_info response
    pub fn get_info(
        pubkey: String,
        network: String,
        block_height: u64,
        methods: Vec<String>,
        notifications: Vec<String>,
    ) -> Self {
        Self {
            result_type: "get_info".to_string(),
            error: None,
            result: Some(serde_json::json!({
                "alias": "Nanduti",
                "color": "#FF6B00",
                "pubkey": pubkey,
                "network": network,
                "block_height": block_height,
                "block_hash": "000000000000000000000000000000000000000000000000000000000000000",
                "methods": methods,
                "notifications": notifications,
            })),
        }
    }

    /// Create an error response
    pub fn error(result_type: String, code: NwcErrorCode, message: String) -> Self {
        Self {
            result_type,
            error: Some(NwcError {
                code: code.as_str().to_string(),
                message,
            }),
            result: None,
        }
    }
}

/// NWC pay_invoice parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayInvoiceParams {
    pub invoice: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<u64>, // msats
}

/// NWC make_invoice parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakeInvoiceParams {
    pub amount: u64, // msats
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiry: Option<u64>, // seconds
}

/// NWC list_transactions parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListTransactionsParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<u64>, // timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>, // timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unpaid: Option<bool>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub transaction_type: Option<String>, // "incoming" or "outgoing"
}

/// NWC pay_keysend parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayKeysendParams {
    pub amount: u64, // msats
    pub pubkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preimage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tlv_records: Option<Vec<TlvRecord>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlvRecord {
    #[serde(rename = "type")]
    pub tlv_type: u64,
    pub value: String, // hex encoded
}
