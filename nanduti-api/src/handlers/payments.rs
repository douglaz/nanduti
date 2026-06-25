//! Payment handlers

use axum::{extract::State, http::StatusCode, Json};
use nanduti_core::models::{
    Amount, Bolt11String, FederationId, PaymentHash, Preimage, Timestamp, TransactionId,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct PayInvoiceRequest {
    pub federation_id: Option<FederationId>,
    pub invoice: Bolt11String,
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
    let invoice = nanduti_core::lightning::LightningOperation::parse_invoice(req.invoice.as_str())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid invoice: {e}")))?;

    // Select federation
    let federation = if let Some(fed_id) = req.federation_id {
        state
            .federation_manager
            .get_federation(&fed_id)
            .await
            .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?
    } else {
        // Use router to select best federation, filtering by the invoice's network
        let amount = invoice
            .amount
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invoice has no amount".to_string()))?;
        state
            .router
            .select_federation_filtered(amount, None, invoice.network)
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

    use nanduti_core::models::{Transaction, TransactionState, TransactionType};

    // Atomic duplicate-payment check: hold the shared in-flight lock across
    // the storage check and pending tx write to prevent concurrent requests
    // for the same invoice from both passing.
    let ph = invoice.payment_hash.to_string();
    {
        let mut in_flight = state.in_flight_payments.lock().await;

        if in_flight.contains(&ph) {
            return Err((
                StatusCode::CONFLICT,
                "Payment already in progress (concurrent request)".to_string(),
            ));
        }

        let existing_txs = state
            .storage
            .get_transactions_by_payment_hash(&invoice.payment_hash)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // Only check outgoing transactions — incoming invoices with the same
        // payment hash should not block outgoing payments (e.g. rebalancing).
        for tx in existing_txs
            .into_iter()
            .filter(|tx| tx.transaction_type == TransactionType::Outgoing)
        {
            if tx.state == TransactionState::Settled {
                return Err((
                    StatusCode::CONFLICT,
                    format!("Invoice already paid (transaction {})", tx.id.as_str()),
                ));
            } else if tx.state == TransactionState::Pending {
                return Err((
                    StatusCode::CONFLICT,
                    format!(
                        "Payment already in progress (transaction {})",
                        tx.id.as_str()
                    ),
                ));
            }
        }

        in_flight.insert(ph.clone());
    }

    // Store initial transaction record before payment
    let uuid = uuid::Uuid::new_v4();
    let transaction_id = TransactionId::new(format!("tx_{uuid}"));
    let created_at = Timestamp::now();

    let mut transaction = Transaction {
        id: transaction_id.clone(),
        federation_id: federation.id.clone(),
        transaction_type: TransactionType::Outgoing,
        state: TransactionState::Pending,
        invoice: Some(req.invoice.clone()),
        amount: invoice.amount.unwrap_or(Amount::from_msats(0)),
        description: invoice.description.clone(),
        payment_hash: invoice.payment_hash.clone(),
        preimage: None,
        fees_paid: None,
        metadata: None,
        created_at,
        settled_at: None,
    };

    // Store pending transaction — clean up in-flight marker on failure
    if let Err(e) = state.storage.store_transaction(&transaction) {
        state.in_flight_payments.lock().await.remove(&ph);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }

    // Pay the invoice
    let result = match client.pay_invoice(&invoice, invoice.amount).await {
        Ok(result) => result,
        Err(e) => {
            // Mark transaction as Failed so it doesn't look in-flight forever
            transaction.state = TransactionState::Failed;
            let _ = state.storage.store_transaction(&transaction);
            // Remove from in-flight set
            state.in_flight_payments.lock().await.remove(&ph);
            return Err((StatusCode::PAYMENT_REQUIRED, e.to_string()));
        }
    };

    // Update transaction with settlement details
    transaction.state = TransactionState::Settled;
    transaction.amount = result.amount_paid;
    transaction.preimage = Some(result.preimage.clone());
    transaction.fees_paid = result.fees_paid;
    transaction.settled_at = Some(Timestamp::now());

    // Best-effort update: the Lightning payment already succeeded, so we must
    // return success to avoid callers retrying a payment that already left the
    // wallet. Log the error for reconciliation but don't fail the response.
    if let Err(e) = state.storage.store_transaction(&transaction) {
        tracing::error!(
            "Failed to persist settled payment {ph}: {e}. Payment was sent successfully."
        );
    }

    // Refresh the federation's cached balance so subsequent routing and
    // balance queries reflect the spend immediately.
    let _ = state
        .federation_manager
        .update_balance(&federation.id)
        .await;

    // Remove from in-flight set
    state.in_flight_payments.lock().await.remove(&ph);

    Ok(Json(PayInvoiceResponse {
        payment_hash: result.payment_hash,
        preimage: result.preimage,
        amount_paid: result.amount_paid,
        fees_paid: result.fees_paid,
        federation_id: federation.id,
    }))
}
