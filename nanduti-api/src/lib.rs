pub mod encryption;
pub mod handlers;
pub mod nostr_client;
pub mod nwc_handler;
pub mod router;
pub mod server;
pub mod state;
pub mod types;

#[cfg(feature = "mcp")]
pub mod mcp_server;

pub use nostr_client::NostrClient;
pub use nwc_handler::NwcHandler;
pub use router::{FederationRouter, RoutingStrategy};
pub use server::Server;
pub use state::AppState;
pub use types::*;

use anyhow::Result;
use nanduti_core::models::Amount;
use std::sync::Arc;
use tracing::info;

/// Start the NWC API server
pub async fn start_server(config: ServerConfig) -> Result<()> {
    info!("Starting NWC API server on {}:{}", config.host, config.port);

    // Create application state with all components
    let app_state = Arc::new(
        AppState::new(
            config.data_dir.clone(),
            config.relays.clone(),
            config.routing_strategy,
        )
        .await?,
    );

    // Publish info event
    publish_info_event(&app_state.nostr_client).await?;

    // Start Nostr event handling loop in background
    let event_handler = app_state.nwc_handler.clone();
    let nostr_clone = app_state.nostr_client.clone();
    tokio::spawn(async move {
        if let Err(e) = handle_nostr_events(nostr_clone, event_handler).await {
            tracing::error!("Nostr event handler error: {e}");
        }
    });

    // Create HTTP server with REST API routes
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.port));
    let http_router = create_http_router(app_state.clone());
    let server = Server::new(http_router, addr);

    info!("NWC server started successfully");
    info!("Wallet public key: {}", app_state.nostr_client.public_key());
    info!("Listening on: {addr}");

    // Run the HTTP server
    server.run().await?;

    Ok(())
}

/// Publish NIP-47 info event
async fn publish_info_event(client: &NostrClient) -> Result<()> {
    // This will be implemented in the nostr_client module
    client.publish_info_event().await
}

/// Handle incoming Nostr events
async fn handle_nostr_events(client: Arc<NostrClient>, handler: Arc<NwcHandler>) -> Result<()> {
    // This will be implemented when we complete the Nostr event loop
    client.handle_nwc_events(handler).await
}

use serde::Serialize;

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

/// Create HTTP router with all REST API endpoints
fn create_http_router(app_state: Arc<AppState>) -> axum::Router {
    use axum::{
        routing::{delete, get, post},
        Json,
    };

    axum::Router::new()
        // Health check
        .route(
            "/health",
            get(|| async { Json(HealthResponse { status: "ok" }) }),
        )
        // Federation management endpoints
        .route(
            "/api/v1/federations",
            get(handlers::federations::list_federations)
                .post(handlers::federations::add_federation),
        )
        .route(
            "/api/v1/federations/{id}",
            get(handlers::federations::get_federation),
        )
        .route(
            "/api/v1/federations/{id}",
            delete(handlers::federations::remove_federation),
        )
        .route(
            "/api/v1/federations/{id}/balance",
            get(handlers::federations::get_federation_balance),
        )
        .route(
            "/api/v1/federations/{id}/gateways",
            get(handlers::federations::list_federation_gateways),
        )
        // Invoice endpoints
        .route("/api/v1/invoices", post(handlers::invoices::create_invoice))
        // Payment endpoints
        .route("/api/v1/payments", post(handlers::payments::pay_invoice))
        // Transaction endpoints
        .route(
            "/api/v1/transactions",
            get(handlers::transactions::list_transactions),
        )
        // NWC connection endpoints
        .route(
            "/api/v1/nwc/connections",
            get(handlers::nwc::list_nwc_connections).post(handlers::nwc::create_nwc_connection),
        )
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
}
