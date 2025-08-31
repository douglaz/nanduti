//! Federation management handlers

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct AddFederationRequest {
    pub invite_code: String,
}

#[derive(Debug, Serialize)]
pub struct AddFederationResponse {
    pub federation_id: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct FederationInfo {
    pub id: String,
    pub name: String,
    pub balance_sats: u64,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct GatewayInfo {
    pub gateway_id: String,
    pub api: String,
    pub base_fee_msat: u32,
    pub proportional_fee_ppm: u32,
    pub vetted: bool,
}

/// Add a new federation
pub async fn add_federation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddFederationRequest>,
) -> Result<Json<AddFederationResponse>, (StatusCode, String)> {
    let federation_id = state
        .federation_manager
        .add_federation(&req.invite_code)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Get federation details for response
    let federation = state
        .federation_manager
        .get_federation(&federation_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(AddFederationResponse {
        federation_id,
        name: federation.name,
    }))
}

/// List all federations
pub async fn list_federations(State(state): State<Arc<AppState>>) -> Json<Vec<FederationInfo>> {
    let federations = state.federation_manager.list_federations().await;

    let infos: Vec<FederationInfo> = federations
        .into_iter()
        .map(|f| FederationInfo {
            id: f.id,
            name: f.name,
            balance_sats: f.balance.as_sats(),
            status: format!("{:?}", f.status),
        })
        .collect();

    Json(infos)
}

/// Get federation details
pub async fn get_federation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<FederationInfo>, (StatusCode, String)> {
    let federation = state
        .federation_manager
        .get_federation(&id)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    Ok(Json(FederationInfo {
        id: federation.id,
        name: federation.name,
        balance_sats: federation.balance.as_sats(),
        status: format!("{:?}", federation.status),
    }))
}

/// Remove a federation
pub async fn remove_federation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .federation_manager
        .remove_federation(&id)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Get federation balance
pub async fn get_federation_balance(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let balance = state
        .federation_manager
        .update_balance(&id)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "federation_id": id,
        "balance_sats": balance.as_sats(),
        "balance_msats": balance.as_msats(),
    })))
}

/// List federation gateways
pub async fn list_federation_gateways(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<GatewayInfo>>, (StatusCode, String)> {
    let federation = state
        .federation_manager
        .get_federation(&id)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    if let Some(client) = federation.client {
        let gateways_with_status = client
            .fetch_gateways_with_vetted_status()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let infos: Vec<GatewayInfo> = gateways_with_status
            .into_iter()
            .map(|(g, is_vetted)| GatewayInfo {
                gateway_id: g.info.gateway_id.to_string(),
                api: g.info.api.to_string(),
                base_fee_msat: g.info.fees.base_msat,
                proportional_fee_ppm: g.info.fees.proportional_millionths,
                vetted: is_vetted,
            })
            .collect();

        Ok(Json(infos))
    } else {
        Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Federation offline".to_string(),
        ))
    }
}
