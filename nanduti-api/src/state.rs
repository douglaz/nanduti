//! Application state for the server

use anyhow::{Context, Result};
use nanduti_core::{
    federation::FederationManager, mnemonic_store::MnemonicStore, storage::Storage,
};
use std::sync::Arc;
use tracing::info;

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
        // Derive storage encryption key from mnemonic if data_dir is set
        let encryption_key = if let Some(ref dir) = data_dir {
            // Ensure data directory exists before any I/O
            std::fs::create_dir_all(dir)
                .with_context(|| format!("Failed to create data directory: {}", dir.display()))?;

            // Get password from environment variable
            let password = std::env::var("NANDUTI_MNEMONIC_PASSWORD")
                .context("NANDUTI_MNEMONIC_PASSWORD environment variable not set")?;

            // Load or generate mnemonic
            let mnemonic =
                if let Some(m) = MnemonicStore::load_mnemonic(dir, Some(&password)).await? {
                    info!("Loaded existing mnemonic for storage encryption");
                    m
                } else {
                    // Generate new mnemonic
                    info!("Generating new mnemonic for storage encryption");
                    let entropy = rand::random::<[u8; 16]>();
                    let mnemonic = fedimint_bip39::Mnemonic::from_entropy(&entropy)?;
                    MnemonicStore::store_mnemonic(dir, &mnemonic, Some(&password)).await?;
                    mnemonic
                };

            // Derive storage encryption key
            let key = MnemonicStore::derive_storage_key(&mnemonic)?;
            info!("Derived storage encryption key from mnemonic");
            Some(key)
        } else {
            None
        };

        // Create storage with encryption key
        let storage = Arc::new(Storage::new(data_dir.as_deref(), encryption_key)?);

        // Create and load federation manager with existing federations
        let federation_manager = Arc::new(
            FederationManager::new_with_load(Some(storage.clone()), data_dir.clone()).await?,
        );

        // Create router
        let router = Arc::new(FederationRouter::new(
            federation_manager.clone(),
            routing_strategy,
        ));

        // Create Nostr client for wallet service
        // Derive wallet keys deterministically from mnemonic so they persist across restarts.
        // Without this, NWC connection URIs (which embed the wallet pubkey) would break on restart.
        let wallet_secret = if let Some(ref dir) = data_dir {
            let password = std::env::var("NANDUTI_MNEMONIC_PASSWORD")
                .context("NANDUTI_MNEMONIC_PASSWORD environment variable not set")?;
            let mnemonic = MnemonicStore::load_mnemonic(dir, Some(&password))
                .await?
                .context("Mnemonic not found - storage should have been initialized above")?;
            let secret_hex = MnemonicStore::derive_nostr_wallet_key(&mnemonic)?;
            info!("Derived wallet Nostr key from mnemonic");
            Some(secret_hex)
        } else {
            // No persistent storage - generate ephemeral keys (development/testing only)
            let keys = nanduti_core::keys::NwcKeys::generate()?;
            Some(keys.secret_key)
        };
        let nostr_client = Arc::new(NostrClient::new(relays, wallet_secret).await?);

        // Create NWC handler
        let nwc_handler = Arc::new(NwcHandler::new(
            federation_manager.clone(),
            router.clone(),
            Some(storage.clone()),
            nostr_client.clone(),
        ));

        Ok(Self {
            federation_manager,
            storage,
            nwc_handler,
            nostr_client,
            router,
        })
    }
}
