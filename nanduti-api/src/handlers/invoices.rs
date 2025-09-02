//! Invoice creation handlers

use axum::{extract::State, http::StatusCode, Json};
use nanduti_core::models::{
    Amount, Bolt11String, Description, PaymentHash, Timestamp, TransactionId,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateInvoiceRequest {
    pub federation_id: Option<String>,
    pub amount: String, // Flexible amount parsing (e.g., "100sats", "0.001btc")
    pub description: String,
    pub expiry: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct CreateInvoiceResponse {
    pub invoice: Bolt11String,
    pub payment_hash: PaymentHash,
    pub amount: Amount,
    pub federation_id: String, // From federation, already a String
}

/// Create a Lightning invoice
pub async fn create_invoice(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateInvoiceRequest>,
) -> Result<Json<CreateInvoiceResponse>, (StatusCode, String)> {
    // Parse amount
    let amount = Amount::from_str(&req.amount)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid amount: {}", e)))?;

    // Select federation
    let federation = if let Some(fed_id) = req.federation_id {
        state
            .federation_manager
            .get_federation(&fed_id)
            .await
            .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?
    } else {
        // Select first online federation
        let federations = state.federation_manager.list_federations().await;
        federations
            .into_iter()
            .find(|f| f.status == nanduti_core::federation::FederationStatus::Online)
            .ok_or_else(|| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "No online federations".to_string(),
                )
            })?
    };

    // Get client
    let client = federation.client.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Federation client not available".to_string(),
        )
    })?;

    // Create invoice
    let description = req.description.clone();
    let invoice = client
        .make_invoice(amount, req.description, req.expiry)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Store transaction record
    use nanduti_core::models::{Transaction, TransactionState, TransactionType};
    let transaction = Transaction {
        id: TransactionId(format!("tx_{}", uuid::Uuid::new_v4())),
        federation_id: federation.id.clone(),
        transaction_type: TransactionType::Incoming,
        state: TransactionState::Pending,
        invoice: Some(invoice.bolt11.clone()),
        amount,
        description: Some(Description(description)),
        payment_hash: invoice.payment_hash.clone(),
        preimage: None,
        fees_paid: None,
        metadata: None,
        created_at: Timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        ),
        settled_at: None,
    };
    state
        .storage
        .store_transaction(&transaction)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(CreateInvoiceResponse {
        invoice: invoice.bolt11,
        payment_hash: invoice.payment_hash,
        amount,
        federation_id: federation.id.0,
    }))
}
