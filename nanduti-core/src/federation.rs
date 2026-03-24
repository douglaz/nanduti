//! Federation management for multi-federation support

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use strum::{Display, EnumString};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::fedimint_client::FedimintClientWrapper;
use crate::models::{Amount, FederationId, FederationMetrics, FederationName, Timestamp};
use crate::storage::Storage;

/// A single federation instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Federation {
    /// Federation ID from invite code
    pub id: FederationId,
    /// Federation name from configuration
    pub name: FederationName,
    /// Original invite code
    pub invite_code: fedimint_core::invite_code::InviteCode,
    /// Current balance in this federation
    pub balance: Amount,
    /// Federation status
    pub status: FederationStatus,
    /// Bitcoin network this federation operates on
    #[serde(default)]
    pub network: crate::nwc_protocol::NwcNetwork,
    /// Performance metrics
    pub metrics: FederationMetrics,
    /// Client wrapper (not serialized)
    #[serde(skip)]
    pub client: Option<Arc<FedimintClientWrapper>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "PascalCase")]
pub enum FederationStatus {
    Online,
    Offline,
    Degraded,
    Initializing,
}

/// Manages multiple federations
pub struct FederationManager {
    federations: Arc<RwLock<HashMap<FederationId, Arc<Federation>>>>,
    storage: Option<Arc<Storage>>,
    data_dir: Option<std::path::PathBuf>,
}

impl FederationManager {
    /// Create a new federation manager
    pub fn new(storage: Option<Arc<Storage>>, data_dir: Option<std::path::PathBuf>) -> Self {
        Self {
            federations: Arc::new(RwLock::new(HashMap::new())),
            storage,
            data_dir,
        }
    }

    /// Create a new federation manager and load existing federations
    /// This method is resilient to storage failures and will start with an empty
    /// federation list if loading fails, rather than failing entirely
    pub async fn new_with_load(
        storage: Option<Arc<Storage>>,
        data_dir: Option<std::path::PathBuf>,
    ) -> Result<Self> {
        let manager = Self::new(storage.clone(), data_dir.clone());

        // Load existing federations from storage
        if let Some(storage) = &storage {
            match storage.list_federations() {
                Ok(stored_federations) => {
                    let mut federations = manager.federations.write().await;

                    for mut federation in stored_federations {
                        let federation_id = &federation.id;
                        let federation_name = &federation.name;
                        info!("Loading federation: {federation_id} ({federation_name})");

                        // Re-initialize the client for each federation
                        match FedimintClientWrapper::new(
                            &federation.invite_code,
                            data_dir.as_deref(),
                        )
                        .await
                        {
                            Ok(client) => {
                                // Update balance with proper error handling
                                match client.get_balance().await {
                                    Ok(balance) => {
                                        federation.balance = balance;
                                        federation.status = FederationStatus::Online;
                                    }
                                    Err(e) => {
                                        warn!(
                                            "Failed to get balance for federation {}: {e}",
                                            federation.id
                                        );
                                        federation.balance = Amount::from_msats(0);
                                        federation.status = FederationStatus::Degraded;
                                    }
                                }
                                // Get network info
                                if let Ok(info) = client.get_info().await {
                                    federation.network =
                                        crate::nwc_protocol::NwcNetwork::from_str_loose(
                                            &info.network,
                                        );
                                }
                                federation.client = Some(Arc::new(client));
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to initialize client for federation {}: {e}",
                                    federation.id
                                );
                                federation.status = FederationStatus::Offline;
                            }
                        }

                        federations.insert(federation.id.clone(), Arc::new(federation));
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to load federations from storage: {e}. Starting with empty federation list."
                    );
                }
            }
        }

        Ok(manager)
    }

    /// Add a new federation from invite code
    pub async fn add_federation(
        &self,
        invite_code: &fedimint_core::invite_code::InviteCode,
    ) -> Result<FederationId> {
        info!("Adding federation from invite code");

        // Parse invite code to get federation ID, name, and the parsed invite
        let (federation_id, federation_name, invite) = self.parse_invite_code(invite_code)?;

        // Check if federation already exists
        {
            let federations = self.federations.read().await;
            if federations.contains_key(&federation_id) {
                bail!("Federation {federation_id} already exists");
            }
        }

        // Create federation entry
        let mut federation = Federation {
            id: federation_id.clone(),
            name: federation_name.clone(),
            invite_code: invite.clone(),
            balance: Amount::from_msats(0),
            status: FederationStatus::Initializing,
            network: crate::nwc_protocol::NwcNetwork::Mainnet,
            metrics: FederationMetrics {
                uptime_percent: 100.0,
                success_rate: 100.0,
                average_fee: Amount::from_msats(0),
                average_latency_ms: 0,
                total_payments: 0,
                total_volume: Amount::from_msats(0),
                last_updated: Timestamp::now(),
            },
            client: None,
        };

        // Initialize Fedimint client
        let client = FedimintClientWrapper::new(&invite, self.data_dir.as_deref())
            .await
            .context("Failed to initialize Fedimint client")?;

        // Get initial balance
        federation.balance = client.get_balance().await?;
        federation.status = FederationStatus::Online;

        // Get network from federation info
        match client.get_info().await {
            Ok(info) => {
                federation.network = crate::nwc_protocol::NwcNetwork::from_str_loose(&info.network);
            }
            Err(e) => {
                warn!("Failed to get network info: {e}, defaulting to mainnet");
            }
        }

        federation.client = Some(Arc::new(client));

        // Persist BEFORE updating the in-memory cache so a storage failure
        // doesn't leave the live state diverged from what will be reloaded.
        let federation_arc = Arc::new(federation);
        if let Some(storage) = &self.storage {
            storage.store_federation(&federation_arc)?;
        }

        // Update in-memory cache only after persistence succeeds
        {
            let mut federations = self.federations.write().await;
            federations.insert(federation_id.clone(), federation_arc.clone());
        }

        info!(
            "Successfully added federation: {federation_id} ({federation_name})",
            federation_name = federation_arc.name
        );
        Ok(federation_id)
    }

    /// Remove a federation
    pub async fn remove_federation(&self, federation_id: &FederationId) -> Result<()> {
        // Verify the federation exists before attempting removal
        {
            let federations = self.federations.read().await;
            if !federations.contains_key(federation_id) {
                bail!("Federation {federation_id} not found");
            }
        }

        // Persist removal BEFORE updating the in-memory cache
        if let Some(storage) = &self.storage {
            storage.remove_federation(federation_id)?;
        }

        // Remove from in-memory cache only after persistence succeeds
        {
            let mut federations = self.federations.write().await;
            if let Some(federation) = federations.remove(federation_id) {
                if let Some(_client) = &federation.client {
                    debug!("Cleaning up federation client for {federation_id}");
                }
            }
        }

        info!("Removed federation: {federation_id}");
        Ok(())
    }

    /// List all federations
    pub async fn list_federations(&self) -> Vec<Federation> {
        let federations = self.federations.read().await;
        federations.values().map(|f| f.as_ref().clone()).collect()
    }

    /// Get a specific federation
    pub async fn get_federation(&self, federation_id: &FederationId) -> Result<Federation> {
        let federations = self.federations.read().await;
        federations
            .get(federation_id)
            .map(|f| f.as_ref().clone())
            .ok_or_else(|| anyhow!("Federation {federation_id} not found"))
    }

    /// Get aggregate balance across all federations
    pub async fn get_total_balance(&self) -> Amount {
        let federations = self.federations.read().await;
        let total_msats: u64 = federations
            .values()
            .filter(|f| f.status == FederationStatus::Online)
            .map(|f| f.balance.as_msats())
            .sum();
        Amount::from_msats(total_msats)
    }

    /// Update federation balance
    ///
    /// # Concurrency
    /// Uses atomic entry API to prevent race conditions. If the federation
    /// is removed during the update, this method will fail with an error
    /// instead of silently re-adding it.
    pub async fn update_balance(&self, federation_id: &FederationId) -> Result<Amount> {
        let mut federations = self.federations.write().await;

        let federation_arc = federations
            .get(federation_id)
            .ok_or_else(|| anyhow!("Federation {federation_id} not found"))?;

        // Get the client before creating new federation to avoid clone overhead
        let client = federation_arc
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("Federation {federation_id} client not initialized"))?;

        // Fetch new balance
        let new_balance = client.get_balance().await?;

        // Create updated federation by cloning the old one and updating balance
        let mut updated_federation = federation_arc.as_ref().clone();
        updated_federation.balance = new_balance;

        // Replace the Arc entirely to avoid make_mut issues
        let updated_arc = Arc::new(updated_federation);

        // Persist to storage first before updating in-memory state
        // This ensures we don't have inconsistency if persistence fails
        if let Some(storage) = &self.storage {
            storage.store_federation(&updated_arc)?;
        }

        // Use entry API for atomic update - prevents race conditions
        // If federation was removed during update, fail instead of re-adding
        match federations.entry(federation_id.clone()) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.insert(updated_arc);
            }
            std::collections::hash_map::Entry::Vacant(_) => {
                bail!("Federation {federation_id} was removed during balance update");
            }
        }

        Ok(new_balance)
    }

    /// Update federation metrics
    ///
    /// # Concurrency
    /// Uses atomic entry API to prevent race conditions. See `update_balance` for details.
    pub async fn update_metrics(
        &self,
        federation_id: &FederationId,
        metrics: FederationMetrics,
    ) -> Result<()> {
        let mut federations = self.federations.write().await;

        let federation_arc = federations
            .get(federation_id)
            .ok_or_else(|| anyhow!("Federation {federation_id} not found"))?;

        // Create updated federation by cloning the old one and updating metrics
        let mut updated_federation = federation_arc.as_ref().clone();
        updated_federation.metrics = metrics;

        // Replace the Arc entirely to avoid make_mut issues
        let updated_arc = Arc::new(updated_federation);

        // Persist to storage first before updating in-memory state
        // This ensures we don't have inconsistency if persistence fails
        if let Some(storage) = &self.storage {
            storage.store_federation(&updated_arc)?;
        }

        // Use entry API for atomic update - prevents race conditions
        match federations.entry(federation_id.clone()) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.insert(updated_arc);
            }
            std::collections::hash_map::Entry::Vacant(_) => {
                bail!("Federation {federation_id} was removed during metrics update");
            }
        }

        Ok(())
    }

    /// Check federation health
    pub async fn check_health(&self, federation_id: &FederationId) -> Result<FederationStatus> {
        let federations = self.federations.read().await;

        let federation = federations
            .get(federation_id)
            .ok_or_else(|| anyhow!("Federation {federation_id} not found"))?;

        if let Some(client) = &federation.client {
            // Try to get balance as a health check
            match client.get_balance().await {
                Ok(_) => Ok(FederationStatus::Online),
                Err(e) => {
                    warn!("Federation {federation_id} health check failed: {e}");
                    Ok(FederationStatus::Degraded)
                }
            }
        } else {
            Ok(FederationStatus::Offline)
        }
    }

    /// Parse invite code to extract federation ID, name, and the parsed invite
    fn parse_invite_code(
        &self,
        invite_code: &fedimint_core::invite_code::InviteCode,
    ) -> Result<(
        FederationId,
        FederationName,
        fedimint_core::invite_code::InviteCode,
    )> {
        let federation_id_str = invite_code.federation_id().to_string();
        // Use plain new() since federation ID from fedimint-core is already validated
        let federation_id = FederationId::new(federation_id_str.clone());

        // Try to extract federation name from the invite code
        // This would typically come from the federation config after joining
        let federation_prefix = &federation_id_str[0..8.min(federation_id_str.len())];
        let federation_name = FederationName::new(format!("Federation {federation_prefix}"));

        Ok((federation_id, federation_name, invite_code.clone()))
    }

    /// Get federations that can pay a specific amount
    pub async fn get_payable_federations(&self, amount: Amount) -> Vec<Federation> {
        let federations = self.federations.read().await;
        federations
            .values()
            .filter(|f| f.status == FederationStatus::Online && f.balance >= amount)
            .map(|f| f.as_ref().clone())
            .collect()
    }
}
