//! Payment handlers

use axum::{extract::State, http::StatusCode, Json};
use lightning_invoice::Bolt11Invoice;
use nanduti_core::models::{
    Amount, Bolt11String, Description, FederationId, Invoice, PaymentHash, Preimage, PublicKey,
    Timestamp, TransactionId,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct PayInvoiceRequest {
    pub federation_id: Option<String>,
    pub invoice: String,
}

#[derive(Debug, Serialize)]
pub struct PayInvoiceResponse {
    pub payment_hash: PaymentHash,
    pub preimage: Preimage,
    pub amount_paid: Amount,
    pub fees_paid: Option<Amount>,
    pub federation_id: FederationId,
}

/// Pay a Lightning invoice
pub async fn pay_invoice(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PayInvoiceRequest>,
) -> Result<Json<PayInvoiceResponse>, (StatusCode, String)> {
    // Parse the invoice
    let bolt11 = Bolt11Invoice::from_str(&req.invoice)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid invoice: {}", e)))?;

    let invoice = Invoice {
        bolt11: Bolt11String(req.invoice.clone()),
        payment_hash: PaymentHash(hex::encode(bolt11.payment_hash().as_ref() as &[u8])),
        amount: bolt11.amount_milli_satoshis().map(Amount::from_msats),
        description: match bolt11.description() {
            lightning_invoice::Bolt11InvoiceDescriptionRef::Direct(desc) => {
                Some(Description(desc.to_string()))
            }
            lightning_invoice::Bolt11InvoiceDescriptionRef::Hash(_) => None,
        },
        expiry: None,
        payee_pubkey: bolt11.payee_pub_key().map(|k| PublicKey(k.to_string())),
    };

    // Select federation
    let federation = if let Some(fed_id) = req.federation_id {
        state
            .federation_manager
            .get_federation(&fed_id)
            .await
            .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?
    } else {
        // Use router to select best federation
        let amount = invoice
            .amount
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invoice has no amount".to_string()))?;
        state
            .router
            .select_federation(amount)
            .await
            .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?
    };

    // Get client
    let client = federation.client.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Federation client not available".to_string(),
        )
    })?;

    // Pay the invoice
    let result = client
        .pay_invoice(&invoice)
        .await
        .map_err(|e| (StatusCode::PAYMENT_REQUIRED, e.to_string()))?;

    // Store transaction record
    use nanduti_core::models::{Transaction, TransactionState, TransactionType};
    let transaction = Transaction {
        id: TransactionId(format!("tx_{}", uuid::Uuid::new_v4())),
        federation_id: federation.id.clone(),
        transaction_type: TransactionType::Outgoing,
        state: TransactionState::Settled,
        invoice: Some(Bolt11String(req.invoice)),
        amount: result.amount_paid,
        description: invoice.description,
        payment_hash: result.payment_hash.clone(),
        preimage: Some(result.preimage.clone()),
        fees_paid: result.fees_paid,
        metadata: None,
        created_at: Timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        ),
        settled_at: Some(Timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )),
    };
    state
        .storage
        .store_transaction(&transaction)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(PayInvoiceResponse {
        payment_hash: result.payment_hash,
        preimage: result.preimage,
        amount_paid: result.amount_paid,
        fees_paid: result.fees_paid,
        federation_id: federation.id,
    }))
}
