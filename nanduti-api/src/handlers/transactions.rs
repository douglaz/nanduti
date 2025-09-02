//! Transaction handlers

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListTransactionsQuery {
    pub federation_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct TransactionInfo {
    pub id: String,
    pub federation_id: String,
    pub transaction_type: String,
    pub state: String,
    pub amount_sats: u64,
    pub description: Option<String>,
    pub payment_hash: String,
    pub created_at: u64,
    pub settled_at: Option<u64>,
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
            .get_federation_transactions(&federation_id, params.limit)
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

    // Sort by created_at descending
    all_transactions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    // Apply limit if specified
    if let Some(limit) = params.limit {
        all_transactions.truncate(limit);
    }

    // Convert to response format
    let infos: Vec<TransactionInfo> = all_transactions
        .into_iter()
        .map(|tx| TransactionInfo {
            id: tx.id.to_string(),
            federation_id: tx.federation_id,
            transaction_type: format!("{:?}", tx.transaction_type),
            state: format!("{:?}", tx.state),
            amount_sats: tx.amount.as_sats(),
            description: tx.description.map(|d| d.0),
            payment_hash: tx.payment_hash.0,
            created_at: tx.created_at.0,
            settled_at: tx.settled_at.map(|t| t.0),
        })
        .collect();

    Json(infos)
}
