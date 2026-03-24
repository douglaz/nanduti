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
        // Use the receive router for proper federation selection policy
        // instead of arbitrarily picking the first online federation.
        state
            .router
            .select_federation_for_receive()
            .await
            .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?
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
        // Persist operation_id so startup recovery can re-subscribe the
        // settlement watcher if the process restarts before settlement.
        metadata: invoice
            .operation_id
            .as_ref()
            .map(|op_id| serde_json::json!({ "operation_id": op_id })),
        created_at: Timestamp::now(),
        settled_at: None,
    };
    // Best-effort persist: the invoice already exists upstream, so we return
    // it to the caller even if local storage fails. A failed write means the
    // settlement watcher won't start, but the invoice can still be paid.
    if let Err(e) = state.storage.store_transaction(&transaction) {
        tracing::error!(
            "Failed to persist invoice {}: {e}. Invoice was created successfully.",
            invoice.payment_hash
        );
    }

    // Spawn background task to watch for invoice settlement
    if let (Some(op_id), Some(client_ref)) = (&invoice.operation_id, &federation.client) {
        let op_id = op_id.clone();
        let client_ref = client_ref.clone();
        let payment_hash = invoice.payment_hash.clone();
        let fed_id = federation.id.clone();
        let storage = state.storage.clone();
        let fm = state.federation_manager.clone();
        tokio::spawn(async move {
            match client_ref.await_invoice_settlement(&op_id).await {
                Ok(true) => {
                    if let Ok(Some(mut tx)) = storage.get_transaction_by_payment_hash(&payment_hash)
                    {
                        tx.state = TransactionState::Settled;
                        tx.settled_at = Some(Timestamp::now());
                        let _ = storage.store_transaction(&tx);
                    }
                    // Refresh cached balance to reflect received funds
                    let _ = fm.update_balance(&fed_id).await;
                }
                Ok(false) => {
                    // Invoice cancelled/expired — mark as Failed
                    if let Ok(Some(mut tx)) = storage.get_transaction_by_payment_hash(&payment_hash)
                    {
                        tx.state = TransactionState::Failed;
                        let _ = storage.store_transaction(&tx);
                    }
                }
                Err(_) => {}
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
