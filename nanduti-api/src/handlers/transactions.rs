//! Transaction handlers

use axum::{
    extract::{Query, State},
    Json,
};
use nanduti_core::models::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListTransactionsQuery {
    pub federation_id: Option<FederationId>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// Timestamp (seconds since epoch) - only return transactions created at or after this time
    pub from: Option<u64>,
    /// Timestamp (seconds since epoch) - only return transactions created at or before this time
    pub until: Option<u64>,
    /// Filter by unpaid (pending) transactions only
    pub unpaid: Option<bool>,
    /// Filter by transaction type: "incoming" or "outgoing"
    #[serde(rename = "type")]
    pub transaction_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TransactionInfo {
    pub id: TransactionId,
    pub federation_id: FederationId,
    pub transaction_type: TransactionType,
    pub state: TransactionState,
    pub amount: Amount,
    pub description: Option<Description>,
    pub payment_hash: PaymentHash,
    pub created_at: Timestamp,
    pub settled_at: Option<Timestamp>,
}

/// List transactions
pub async fn list_transactions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListTransactionsQuery>,
) -> Json<Vec<TransactionInfo>> {
    let mut all_transactions = Vec::new();

    if let Some(federation_id) = params.federation_id {
        // Get transactions for specific federation
        if let Ok(txs) = state
            .storage
            .get_federation_transactions(&federation_id, None)
        {
            all_transactions.extend(txs);
        }
    } else {
        // Get transactions for all federations
        let federations = state.federation_manager.list_federations().await;
        for federation in federations {
            if let Ok(txs) = state
                .storage
                .get_federation_transactions(&federation.id, None)
            {
                all_transactions.extend(txs);
            }
        }
    }

    // Filter by timestamp range (from/until)
    if let Some(from_ts) = params.from {
        let from = Timestamp::from_secs(from_ts);
        all_transactions.retain(|tx| tx.created_at >= from);
    }
    if let Some(until_ts) = params.until {
        let until = Timestamp::from_secs(until_ts);
        all_transactions.retain(|tx| tx.created_at <= until);
    }

    // Filter by transaction type (incoming/outgoing)
    if let Some(tx_type) = &params.transaction_type {
        all_transactions.retain(|tx| match tx_type.as_str() {
            "incoming" => tx.transaction_type == TransactionType::Incoming,
            "outgoing" => tx.transaction_type == TransactionType::Outgoing,
            _ => true, // Unknown type, don't filter
        });
    }

    // Filter by unpaid status (pending transactions)
    if let Some(true) = params.unpaid {
        all_transactions.retain(|tx| tx.state == TransactionState::Pending);
    }

    // Sort by created_at descending
    all_transactions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    // Apply offset (skip first N transactions)
    if let Some(offset) = params.offset {
        if offset < all_transactions.len() {
            all_transactions = all_transactions.split_off(offset);
        } else {
            all_transactions.clear();
        }
    }

    // Apply limit if specified
    if let Some(limit) = params.limit {
        all_transactions.truncate(limit);
    }

    // Convert to response format
    let infos: Vec<TransactionInfo> = all_transactions
        .into_iter()
        .map(|tx| TransactionInfo {
            id: tx.id,
            federation_id: tx.federation_id,
            transaction_type: tx.transaction_type,
            state: tx.state,
            amount: tx.amount,
            description: tx.description,
            payment_hash: tx.payment_hash,
            created_at: tx.created_at,
            settled_at: tx.settled_at,
        })
        .collect();

    Json(infos)
}
