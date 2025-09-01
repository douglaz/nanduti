//! Application state for the server

use anyhow::Result;
use nanduti_core::{federation::FederationManager, storage::Storage};
use std::sync::Arc;

use crate::{FederationRouter, NostrClient, NwcHandler, RoutingStrategy};

/// Shared application state for all handlers
#[derive(Clone)]
pub struct AppState {
    /// Federation manager (persistent across requests)
    pub federation_manager: Arc<FederationManager>,

    /// Storage backend
    pub storage: Arc<Storage>,

    /// NWC protocol handler
    pub nwc_handler: Arc<NwcHandler>,

    /// Nostr client for NWC
    pub nostr_client: Arc<NostrClient>,

    /// Federation router for payment routing
    pub router: Arc<FederationRouter>,
}

impl AppState {
    /// Create new app state with all components initialized
    pub async fn new(
        data_dir: Option<std::path::PathBuf>,
        relays: Vec<String>,
        routing_strategy: RoutingStrategy,
    ) -> Result<Self> {
        // Create storage
        let storage = Arc::new(Storage::new(data_dir.as_deref())?);

        // Create and load federation manager with existing federations
        let federation_manager = Arc::new(
            FederationManager::new_with_load(Some(storage.clone()), data_dir.clone()).await?,
        );

        // Create router
        let router = Arc::new(FederationRouter::new(
            federation_manager.clone(),
            routing_strategy,
        ));

        // Create NWC handler
        let nwc_handler = Arc::new(NwcHandler::new(
            federation_manager.clone(),
            router.clone(),
            Some(storage.clone()),
        ));

        // Create Nostr client for wallet service
        let wallet_keys = nanduti_core::keys::NwcKeys::generate()?;
        let nostr_client = Arc::new(NostrClient::new(relays, Some(wallet_keys.secret_key)).await?);

        Ok(Self {
            federation_manager,
            storage,
            nwc_handler,
            nostr_client,
            router,
        })
    }
}
