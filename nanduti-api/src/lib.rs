pub mod encryption;
pub mod handlers;
pub mod nostr_client;
pub mod nwc_handler;
pub mod router;
pub mod server;
pub mod state;
pub mod types;

pub use nostr_client::NostrClient;
pub use nwc_handler::NwcHandler;
pub use router::{FederationRouter, RoutingStrategy};
pub use server::Server;
pub use state::AppState;
pub use types::*;

use anyhow::{Context, Result};
use nanduti_core::models::Amount;
use std::sync::Arc;
use tracing::info;

/// Start the NWC API server
pub async fn start_server(config: ServerConfig) -> Result<()> {
    info!(
        "Starting NWC API server on {host}:{port}",
        host = config.host,
        port = config.port
    );

    // Create application state with all components
    let app_state = Arc::new(
        AppState::new(
            config.data_dir.clone(),
            config.relays.clone(),
            config.routing_strategy,
            config.max_payment_amount,
            config.daily_limit_amount,
        )
        .await?,
    );

    // Expire stale pending outgoing transactions from before this startup.
    // Use a generous 24-hour window to avoid prematurely marking slow Fedimint
    // operations as failed, which could enable duplicate payments if the original
    // settles later. The underlying Fedimint operation may still complete; the
    // payment hash index will catch any resulting Settled record.
    match app_state.storage.expire_stale_pending(86400) {
        Ok(0) => {}
        Ok(n) => info!("Expired {n} stale pending outgoing transactions"),
        Err(e) => tracing::warn!("Failed to expire stale pending transactions: {e}"),
    }

    // Re-subscribe pending incoming invoices that have operation_ids in metadata.
    // These settlement watchers were lost when the previous process exited.
    {
        use nanduti_core::models::{TransactionState, TransactionType};
        if let Ok(all_txs) = app_state.storage.get_all_transactions() {
            let pending_incoming: Vec<_> = all_txs
                .into_iter()
                .filter(|tx| {
                    tx.state == TransactionState::Pending
                        && tx.transaction_type == TransactionType::Incoming
                })
                .collect();

            for tx in pending_incoming {
                let op_id = tx
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("operation_id"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                if let Some(op_id) = op_id {
                    // Find the federation's client to re-subscribe
                    if let Ok(federation) = app_state
                        .federation_manager
                        .get_federation(&tx.federation_id)
                        .await
                    {
                        if let Some(client) = &federation.client {
                            let client = client.clone();
                            let payment_hash = tx.payment_hash.clone();
                            let tx_id = tx.id.clone();
                            let fed_id = tx.federation_id.clone();
                            let storage = app_state.storage.clone();
                            let fm = app_state.federation_manager.clone();
                            info!("Re-subscribing settlement watcher for invoice {tx_id}");
                            tokio::spawn(async move {
                                match client.await_invoice_settlement(&op_id).await {
                                    Ok(true) => {
                                        info!("Invoice {tx_id} settled on federation {fed_id}");
                                        if let Ok(Some(mut tx_update)) =
                                            storage.get_transaction_by_payment_hash(&payment_hash)
                                        {
                                            tx_update.state = TransactionState::Settled;
                                            tx_update.settled_at =
                                                Some(nanduti_core::models::Timestamp::now());
                                            let _ = storage.store_transaction(&tx_update);
                                        }
                                        let _ = fm.update_balance(&fed_id).await;
                                    }
                                    Ok(false) => {
                                        if let Ok(Some(mut tx_update)) =
                                            storage.get_transaction_by_payment_hash(&payment_hash)
                                        {
                                            tx_update.state = TransactionState::Failed;
                                            let _ = storage.store_transaction(&tx_update);
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "Failed to re-subscribe invoice {tx_id}: {e}"
                                        );
                                    }
                                }
                            });
                        }
                    }
                }
            }
        }
    }

    // Seed federations from CLI/env invite codes
    for invite_str in &config.federations {
        match std::str::FromStr::from_str(invite_str) {
            Ok(invite_code) => {
                match app_state
                    .federation_manager
                    .add_federation(&invite_code)
                    .await
                {
                    Ok(federation_id) => {
                        info!("Added federation from startup config: {federation_id}");
                    }
                    Err(e) => {
                        // Log but don't fail startup — federation may already exist
                        tracing::warn!("Failed to add federation from invite code: {e}");
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Invalid federation invite code: {e}");
            }
        }
    }

    // Publish info event
    publish_info_event(&app_state.nostr_client).await?;

    // Start Nostr event handling loop in background
    let event_handler = app_state.nwc_handler.clone();
    let nostr_clone = app_state.nostr_client.clone();
    let storage_clone = app_state.storage.clone();
    tokio::spawn(async move {
        if let Err(error) = handle_nostr_events(nostr_clone, event_handler, storage_clone).await {
            tracing::error!("Nostr event handler error: {error}");
        }
    });

    // Warn if no API secret is set and binding to non-loopback
    if config.api_secret.is_none() && config.host != "127.0.0.1" && config.host != "::1" {
        tracing::warn!(
            "REST API is unauthenticated and bound to {}. Set API_SECRET to require bearer token auth.",
            config.host
        );
    }

    // Create HTTP server with REST API routes
    let ip: std::net::IpAddr = config
        .host
        .parse()
        .with_context(|| format!("Invalid host address: {}", config.host))?;
    let addr = std::net::SocketAddr::from((ip, config.port));
    let http_router = create_http_router(app_state.clone(), config.api_secret.clone());
    let server = Server::new(http_router, addr);

    info!("NWC server started successfully");
    info!(
        "Wallet public key: {pubkey}",
        pubkey = app_state.nostr_client.public_key()
    );
    info!("Listening on: {addr}");

    // Run the HTTP server
    server.run().await?;

    Ok(())
}

/// Publish NIP-47 info event
/// Announces the wallet's capabilities and supported NWC methods to the Nostr network
async fn publish_info_event(client: &NostrClient) -> Result<()> {
    client.publish_info_event().await
}

/// Handle incoming Nostr events
/// Starts the Nostr event loop that listens for NWC requests and processes them
async fn handle_nostr_events(
    client: Arc<NostrClient>,
    handler: Arc<NwcHandler>,
    storage: Arc<nanduti_core::storage::Storage>,
) -> Result<()> {
    client.handle_nwc_events(handler, storage).await
}

use serde::Serialize;

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

/// Create HTTP router with all REST API endpoints
fn create_http_router(app_state: Arc<AppState>, api_secret: Option<String>) -> axum::Router {
    use axum::{
        routing::{get, post},
        Json,
    };

    // Authenticated API routes
    let api_routes = axum::Router::new()
        // Federation management endpoints
        .route(
            "/federations",
            get(handlers::federations::list_federations)
                .post(handlers::federations::add_federation),
        )
        .route(
            "/federations/{id}",
            get(handlers::federations::get_federation)
                .delete(handlers::federations::remove_federation),
        )
        .route(
            "/federations/{id}/balance",
            get(handlers::federations::get_federation_balance),
        )
        .route(
            "/federations/{id}/gateways",
            get(handlers::federations::list_federation_gateways),
        )
        // Invoice endpoints
        .route("/invoices", post(handlers::invoices::create_invoice))
        // Payment endpoints
        .route("/payments", post(handlers::payments::pay_invoice))
        // Transaction endpoints
        .route(
            "/transactions",
            get(handlers::transactions::list_transactions),
        )
        // NWC connection endpoints
        .route(
            "/nwc/connections",
            get(handlers::nwc::list_nwc_connections).post(handlers::nwc::create_nwc_connection),
        );

    // Apply bearer token auth middleware when api_secret is configured
    let api_routes = if let Some(secret) = api_secret {
        api_routes.layer(axum::middleware::from_fn(
            move |req: axum::http::Request<axum::body::Body>, next: axum::middleware::Next| {
                let expected = secret.clone();
                async move {
                    let auth_header = req
                        .headers()
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok());

                    match auth_header {
                        Some(header) if header.starts_with("Bearer ") => {
                            let token = &header["Bearer ".len()..];
                            if token == expected {
                                Ok(next.run(req).await)
                            } else {
                                Err(axum::http::StatusCode::UNAUTHORIZED)
                            }
                        }
                        _ => Err(axum::http::StatusCode::UNAUTHORIZED),
                    }
                }
            },
        ))
    } else {
        api_routes
    };

    axum::Router::new()
        // Health check (unauthenticated)
        .route(
            "/health",
            get(|| async { Json(HealthResponse { status: "ok" }) }),
        )
        .nest("/api/v1", api_routes)
        // Add shared state to all routes
        .with_state(app_state)
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub relays: Vec<String>,
    pub data_dir: Option<std::path::PathBuf>,
    pub routing_strategy: RoutingStrategy,
    pub max_payment_amount: Option<Amount>,
    pub daily_limit_amount: Option<Amount>,
    /// Fedimint invite codes to join on startup
    pub federations: Vec<String>,
    /// Optional bearer token for REST API authentication.
    /// When set, all `/api/v1/*` endpoints require `Authorization: Bearer <token>`.
    /// When None, the API is unauthenticated (suitable for loopback-only access).
    pub api_secret: Option<String>,
}
