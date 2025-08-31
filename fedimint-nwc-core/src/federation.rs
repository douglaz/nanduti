//! Federation management for multi-federation support

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::fedimint_client::FedimintClientWrapper;
use crate::models::{Amount, FederationMetrics};
use crate::storage::Storage;

/// A single federation instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Federation {
    /// Federation ID from invite code
    pub id: String,
    /// Federation name from configuration
    pub name: String,
    /// Original invite code
    pub invite_code: String,
    /// Current balance in this federation
    pub balance: Amount,
    /// Federation status
    pub status: FederationStatus,
    /// Performance metrics
    pub metrics: FederationMetrics,
    /// Client wrapper (not serialized)
    #[serde(skip)]
    pub client: Option<Arc<FedimintClientWrapper>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FederationStatus {
    Online,
    Offline,
    Degraded,
    Initializing,
}

/// Manages multiple federations
pub struct FederationManager {
    federations: Arc<RwLock<HashMap<String, Federation>>>,
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
    pub async fn new_with_load(
        storage: Option<Arc<Storage>>,
        data_dir: Option<std::path::PathBuf>,
    ) -> Result<Self> {
        let manager = Self::new(storage.clone(), data_dir.clone());

        // Load existing federations from storage
        if let Some(storage) = &storage {
            let stored_federations = storage.list_federations()?;
            let mut federations = manager.federations.write().await;

            for mut federation in stored_federations {
                info!(
                    "Loading federation: {} ({})",
                    federation.id, federation.name
                );

                // Re-initialize the client for each federation
                if !federation.invite_code.is_empty() {
                    match FedimintClientWrapper::new(&federation.invite_code, data_dir.as_deref())
                        .await
                    {
                        Ok(client) => {
                            // Update balance
                            federation.balance =
                                client.get_balance().await.unwrap_or(Amount::from_msats(0));
                            federation.status = FederationStatus::Online;
                            federation.client = Some(Arc::new(client));
                        }
                        Err(e) => {
                            warn!(
                                "Failed to initialize client for federation {}: {}",
                                federation.id, e
                            );
                            federation.status = FederationStatus::Offline;
                        }
                    }
                }

                federations.insert(federation.id.clone(), federation);
            }
        }

        Ok(manager)
    }

    /// Add a new federation from invite code
    pub async fn add_federation(&self, invite_code: &str) -> Result<String> {
        info!("Adding federation from invite code");

        // Parse invite code to get federation ID and name
        let (federation_id, federation_name) = self.parse_invite_code(invite_code)?;

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
            name: federation_name,
            invite_code: invite_code.to_string(),
            balance: Amount::from_msats(0),
            status: FederationStatus::Initializing,
            metrics: FederationMetrics {
                uptime_percent: 100.0,
                success_rate: 100.0,
                average_fee: Amount::from_msats(0),
                average_latency_ms: 0,
                total_payments: 0,
                total_volume: Amount::from_msats(0),
                last_updated: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            },
            client: None,
        };

        // Initialize Fedimint client
        let client = FedimintClientWrapper::new(invite_code, self.data_dir.as_deref())
            .await
            .context("Failed to initialize Fedimint client")?;

        // Get initial balance
        federation.balance = client.get_balance().await?;
        federation.status = FederationStatus::Online;
        federation.client = Some(Arc::new(client));

        // Store federation
        {
            let mut federations = self.federations.write().await;
            federations.insert(federation_id.clone(), federation.clone());
        }

        // Persist if storage is available
        if let Some(storage) = &self.storage {
            storage.store_federation(&federation)?;
        }

        info!(
            "Successfully added federation: {federation_id} ({federation_name})",
            federation_name = federation.name
        );
        Ok(federation_id)
    }

    /// Remove a federation
    pub async fn remove_federation(&self, federation_id: &str) -> Result<()> {
        let mut federations = self.federations.write().await;

        let federation = federations
            .remove(federation_id)
            .ok_or_else(|| anyhow!("Federation {federation_id} not found"))?;

        // Cleanup client if needed
        if let Some(_client) = federation.client {
            // Perform any cleanup operations
            debug!("Cleaning up federation client for {federation_id}");
        }

        // Remove from storage
        if let Some(storage) = &self.storage {
            storage.remove_federation(federation_id)?;
        }

        info!("Removed federation: {federation_id}");
        Ok(())
    }

    /// List all federations
    pub async fn list_federations(&self) -> Vec<Federation> {
        let federations = self.federations.read().await;
        federations.values().cloned().collect()
    }

    /// Get a specific federation
    pub async fn get_federation(&self, federation_id: &str) -> Result<Federation> {
        let federations = self.federations.read().await;
        federations
            .get(federation_id)
            .cloned()
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
    pub async fn update_balance(&self, federation_id: &str) -> Result<Amount> {
        let mut federations = self.federations.write().await;

        let federation = federations
            .get_mut(federation_id)
            .ok_or_else(|| anyhow!("Federation {federation_id} not found"))?;

        if let Some(client) = &federation.client {
            federation.balance = client.get_balance().await?;

            // Update storage
            if let Some(storage) = &self.storage {
                storage.store_federation(federation)?;
            }

            Ok(federation.balance)
        } else {
            bail!("Federation {federation_id} client not initialized");
        }
    }

    /// Update federation metrics
    pub async fn update_metrics(
        &self,
        federation_id: &str,
        metrics: FederationMetrics,
    ) -> Result<()> {
        let mut federations = self.federations.write().await;

        let federation = federations
            .get_mut(federation_id)
            .ok_or_else(|| anyhow!("Federation {federation_id} not found"))?;

        federation.metrics = metrics;

        // Update storage
        if let Some(storage) = &self.storage {
            storage.store_federation(federation)?;
        }

        Ok(())
    }

    /// Check federation health
    pub async fn check_health(&self, federation_id: &str) -> Result<FederationStatus> {
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

    /// Parse invite code to extract federation ID and name
    fn parse_invite_code(&self, invite_code: &str) -> Result<(String, String)> {
        use fedimint_core::invite_code::InviteCode;
        use std::str::FromStr;

        // Parse the invite code using fedimint-core
        let invite = InviteCode::from_str(invite_code).context("Failed to parse invite code")?;

        let federation_id = invite.federation_id().to_string();

        // Try to extract federation name from the invite code
        // This would typically come from the federation config after joining
        let federation_name = format!(
            "Federation {}",
            &federation_id[0..8.min(federation_id.len())]
        );

        Ok((federation_id, federation_name))
    }

    /// Get federations that can pay a specific amount
    pub async fn get_payable_federations(&self, amount: Amount) -> Vec<Federation> {
        let federations = self.federations.read().await;
        federations
            .values()
            .filter(|f| f.status == FederationStatus::Online && f.balance >= amount)
            .cloned()
            .collect()
    }
}
