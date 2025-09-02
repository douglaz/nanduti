//! NWC connection handlers

use axum::{extract::State, http::StatusCode, Json};
use nanduti_core::models::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateConnectionRequest {
    pub name: ConnectionName,
    pub daily_limit_sats: Option<u64>,
    pub per_payment_limit_sats: Option<u64>,
    pub allowed_federations: Vec<FederationId>, // Federation IDs or ["*"] for all
    pub relays: Vec<RelayUrl>,
    pub lud16: Option<LightningAddress>,
}

#[derive(Debug, Serialize)]
pub struct CreateConnectionResponse {
    pub connection_id: ConnectionId,
    pub name: ConnectionName,
    pub pubkey: PublicKey,
    pub connection_uri: ConnectionUri,
}

#[derive(Debug, Serialize)]
pub struct ConnectionInfo {
    pub id: ConnectionId,
    pub name: ConnectionName,
    pub pubkey: PublicKey,
    pub created_at: Timestamp,
    pub last_used: Option<Timestamp>,
    pub total_spent_msats: u64,
}

/// Create a new NWC connection
pub async fn create_nwc_connection(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateConnectionRequest>,
) -> Result<Json<CreateConnectionResponse>, (StatusCode, String)> {
    use nanduti_core::keys::NwcKeys;
    use nanduti_core::storage::NwcConnection;

    // Generate keys for the connection
    let wallet_keys =
        NwcKeys::generate().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Generate connection URI
    let client_connection = nanduti_core::keys::NwcConnection::generate(
        wallet_keys.public_key.clone(),
        req.relays.iter().map(|r| r.0.clone()).collect(),
        req.lud16.as_ref().map(|l| l.0.clone()),
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Store connection
    let connection = NwcConnection {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name.0.clone(),
        pubkey: wallet_keys.public_key.clone(),
        allowed_federations: req
            .allowed_federations
            .iter()
            .map(|f| f.0.clone())
            .collect(),
        daily_limit_msats: req.daily_limit_sats.map(|s| s * 1000),
        per_payment_limit_msats: req.per_payment_limit_sats.map(|s| s * 1000),
        allowed_methods: vec![
            "pay_invoice".to_string(),
            "make_invoice".to_string(),
            "get_balance".to_string(),
            "list_transactions".to_string(),
        ],
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        last_used: None,
        total_spent_msats: 0,
    };

    state
        .storage
        .store_connection(&connection)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(CreateConnectionResponse {
        connection_id: ConnectionId::new(connection.id),
        name: req.name,
        pubkey: PublicKey::new(wallet_keys.public_key),
        connection_uri: ConnectionUri::new(client_connection.to_uri()),
    }))
}

/// List NWC connections
pub async fn list_nwc_connections(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ConnectionInfo>>, (StatusCode, String)> {
    let connections = state
        .storage
        .list_connections()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let infos: Vec<ConnectionInfo> = connections
        .into_iter()
        .map(|c| ConnectionInfo {
            id: ConnectionId::new(c.id),
            name: ConnectionName::new(c.name),
            pubkey: PublicKey::new(c.pubkey),
            created_at: Timestamp::from_secs(c.created_at),
            last_used: c.last_used.map(Timestamp::from_secs),
            total_spent_msats: c.total_spent_msats,
        })
        .collect();

    Ok(Json(infos))
}
