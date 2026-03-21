//! Invoice creation handlers

use axum::{extract::State, http::StatusCode, Json};
use nanduti_core::models::{
    Amount, Bolt11String, Description, FederationId, PaymentHash, Timestamp, TransactionId,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateInvoiceRequest {
    pub federation_id: Option<FederationId>,
    pub amount: Amount,
    pub description: Description,
    pub expiry: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct CreateInvoiceResponse {
    pub invoice: Bolt11String,
    pub payment_hash: PaymentHash,
    pub amount: Amount,
    pub federation_id: FederationId,
}

/// Create a Lightning invoice
pub async fn create_invoice(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateInvoiceRequest>,
) -> Result<Json<CreateInvoiceResponse>, (StatusCode, String)> {
    // Use the already-parsed amount
    let amount = req.amount;

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
    let client = federation.client.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Federation client not available".to_string(),
        )
    })?;

    // Create invoice
    let description = req.description.clone();
    let invoice = client
        .make_invoice(amount, req.description.into_string(), req.expiry)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Store transaction record
    use nanduti_core::models::{Transaction, TransactionState, TransactionType};
    let transaction = Transaction {
        id: {
            let uuid = uuid::Uuid::new_v4();
            TransactionId::new(format!("tx_{uuid}"))
        },
        federation_id: federation.id.clone(),
        transaction_type: TransactionType::Incoming,
        state: TransactionState::Pending,
        invoice: Some(invoice.bolt11.clone()),
        amount,
        description: Some(description),
        payment_hash: invoice.payment_hash.clone(),
        preimage: None,
        fees_paid: None,
        metadata: None,
        created_at: Timestamp::now(),
        settled_at: None,
    };
    state
        .storage
        .store_transaction(&transaction)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Spawn background task to watch for invoice settlement
    if let (Some(op_id), Some(client_ref)) = (&invoice.operation_id, &federation.client) {
        let op_id = op_id.clone();
        let client_ref = client_ref.clone();
        let payment_hash = invoice.payment_hash.clone();
        let storage = state.storage.clone();
        tokio::spawn(async move {
            if let Ok(true) = client_ref.await_invoice_settlement(&op_id).await {
                if let Ok(Some(mut tx)) = storage.get_transaction_by_payment_hash(&payment_hash) {
                    tx.state = TransactionState::Settled;
                    tx.settled_at = Some(Timestamp::now());
                    let _ = storage.store_transaction(&tx);
                }
            }
        });
    }

    Ok(Json(CreateInvoiceResponse {
        invoice: invoice.bolt11,
        payment_hash: invoice.payment_hash,
        amount,
        federation_id: federation.id,
    }))
}
