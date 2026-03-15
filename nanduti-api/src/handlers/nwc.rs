//! NWC connection handlers

use axum::{extract::State, http::StatusCode, Json};
use nanduti_core::models::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateConnectionRequest {
    pub name: ConnectionName,
    pub daily_limit: Option<Amount>,
    pub per_payment_limit: Option<Amount>,
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
    use nanduti_core::storage::NwcConnection;

    // Use the server's actual wallet pubkey for the connection URI.
    // This is the key the Nostr client listens on, so clients can reach the server.
    let wallet_pubkey = state.nostr_client.public_key();

    // Generate a client secret for this connection. The client will use this
    // secret to sign NWC requests, and we store the derived pubkey for auth lookups.
    let client_connection = nanduti_core::keys::NwcConnection::generate(
        wallet_pubkey.clone(),
        req.relays.iter().map(|r| r.to_string()).collect(),
        req.lud16.as_ref().map(|l| l.to_string()),
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Derive the client's public key from the secret for connection lookup/authorization
    let client_keys = client_connection
        .get_client_keys()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Store connection
    let allowed_federations = if req.allowed_federations.is_empty()
        || req.allowed_federations.iter().any(|f| f.as_str() == "*")
    {
        nanduti_core::models::FederationFilter::All
    } else {
        nanduti_core::models::FederationFilter::Specific(req.allowed_federations.clone())
    };

    let connection = NwcConnection {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name.to_string(),
        pubkey: client_keys.public_key.clone(),
        allowed_federations,
        daily_limit_msats: req.daily_limit.map(|a| a.as_msats()),
        per_payment_limit_msats: req.per_payment_limit.map(|a| a.as_msats()),
        allowed_methods: nanduti_core::models::MethodFilter::specific(vec![
            "pay_invoice".to_string(),
            "make_invoice".to_string(),
            "get_balance".to_string(),
            "list_transactions".to_string(),
        ]),
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0), // Fallback to 0 if system time is before UNIX epoch
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
        pubkey: PublicKey::new(client_keys.public_key),
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
