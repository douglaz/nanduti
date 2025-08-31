//! Storage layer using sled embedded database

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info};

use crate::federation::Federation;
use crate::models::Transaction;

/// Storage backend for persisting federation and transaction data
#[derive(Clone)]
pub struct Storage {
    db: Option<Arc<sled::Db>>,
    federations: Option<sled::Tree>,
    connections: Option<sled::Tree>,
    transactions: Option<sled::Tree>,
}

impl Storage {
    /// Create a new storage instance
    /// If data_dir is None, operates in memory-only mode
    pub fn new(data_dir: Option<&Path>) -> Result<Self> {
        let (db, federations, connections, transactions) = match data_dir {
            Some(dir) => {
                info!("Opening database at {}", dir.display());

                // Create directory if it doesn't exist
                std::fs::create_dir_all(dir).context("Failed to create data directory")?;

                let db_path = dir.join("fedimint-nwc.db");
                let db = sled::open(&db_path)
                    .with_context(|| format!("Failed to open database at {}", db_path.display()))?;

                let federations = Some(
                    db.open_tree("federations")
                        .context("Failed to open federations tree")?,
                );
                let connections = Some(
                    db.open_tree("connections")
                        .context("Failed to open connections tree")?,
                );
                let transactions = Some(
                    db.open_tree("transactions")
                        .context("Failed to open transactions tree")?,
                );

                (Some(Arc::new(db)), federations, connections, transactions)
            }
            None => {
                info!("Running in memory-only mode (no persistence)");
                (None, None, None, None)
            }
        };

        Ok(Self {
            db,
            federations,
            connections,
            transactions,
        })
    }

    /// Store a federation
    pub fn store_federation(&self, federation: &Federation) -> Result<()> {
        if let Some(tree) = &self.federations {
            let data = serde_json::to_vec(federation).context("Failed to serialize federation")?;
            tree.insert(federation.id.as_bytes(), data)
                .context("Failed to store federation")?;
            debug!("Stored federation: {}", federation.id);
        }
        Ok(())
    }

    /// Get a federation by ID
    pub fn get_federation(&self, federation_id: &str) -> Result<Option<Federation>> {
        if let Some(tree) = &self.federations {
            if let Some(data) = tree
                .get(federation_id.as_bytes())
                .context("Failed to read federation")?
            {
                let federation: Federation =
                    serde_json::from_slice(&data).context("Failed to deserialize federation")?;
                return Ok(Some(federation));
            }
        }
        Ok(None)
    }

    /// List all federations
    pub fn list_federations(&self) -> Result<Vec<Federation>> {
        let mut federations = Vec::new();

        if let Some(tree) = &self.federations {
            for item in tree.iter() {
                let (_, value) = item.context("Failed to read federation item")?;
                let federation: Federation =
                    serde_json::from_slice(&value).context("Failed to deserialize federation")?;
                federations.push(federation);
            }
        }

        Ok(federations)
    }

    /// Remove a federation
    pub fn remove_federation(&self, federation_id: &str) -> Result<()> {
        if let Some(tree) = &self.federations {
            tree.remove(federation_id.as_bytes())
                .context("Failed to remove federation")?;
            debug!("Removed federation: {federation_id}");
        }
        Ok(())
    }

    /// Store a transaction
    pub fn store_transaction(&self, transaction: &Transaction) -> Result<()> {
        if let Some(tree) = &self.transactions {
            let data =
                serde_json::to_vec(transaction).context("Failed to serialize transaction")?;
            tree.insert(transaction.id.as_bytes(), data)
                .context("Failed to store transaction")?;
            debug!("Stored transaction: {}", transaction.id);
        }
        Ok(())
    }

    /// Get transactions for a federation
    pub fn get_federation_transactions(
        &self,
        federation_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Transaction>> {
        let mut transactions = Vec::new();

        if let Some(tree) = &self.transactions {
            for item in tree.iter() {
                let (_, value) = item.context("Failed to read transaction item")?;
                let transaction: Transaction =
                    serde_json::from_slice(&value).context("Failed to deserialize transaction")?;

                if transaction.federation_id == federation_id {
                    transactions.push(transaction);

                    if let Some(limit) = limit {
                        if transactions.len() >= limit {
                            break;
                        }
                    }
                }
            }
        }

        // Sort by created_at descending
        transactions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(transactions)
    }

    /// Store a NWC connection
    pub fn store_connection(&self, connection: &NwcConnection) -> Result<()> {
        if let Some(tree) = &self.connections {
            let data = serde_json::to_vec(connection).context("Failed to serialize connection")?;
            tree.insert(connection.id.as_bytes(), data)
                .context("Failed to store connection")?;
            debug!("Stored connection: {}", connection.id);
        }
        Ok(())
    }

    /// Get a NWC connection by public key
    pub fn get_connection(&self, pubkey: &str) -> Result<Option<NwcConnection>> {
        if let Some(tree) = &self.connections {
            for item in tree.iter() {
                let (_, value) = item.context("Failed to read connection item")?;
                let connection: NwcConnection =
                    serde_json::from_slice(&value).context("Failed to deserialize connection")?;

                if connection.pubkey == pubkey {
                    return Ok(Some(connection));
                }
            }
        }
        Ok(None)
    }

    /// List all NWC connections
    pub fn list_connections(&self) -> Result<Vec<NwcConnection>> {
        let mut connections = Vec::new();

        if let Some(tree) = &self.connections {
            for item in tree.iter() {
                let (_, value) = item.context("Failed to read connection item")?;
                let connection: NwcConnection =
                    serde_json::from_slice(&value).context("Failed to deserialize connection")?;
                connections.push(connection);
            }
        }

        Ok(connections)
    }

    /// Flush all pending writes to disk
    pub fn flush(&self) -> Result<()> {
        if let Some(db) = &self.db {
            db.flush().context("Failed to flush database")?;
            debug!("Database flushed to disk");
        }
        Ok(())
    }
}

/// NWC connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NwcConnection {
    pub id: String,
    pub name: String,
    pub pubkey: String,
    pub allowed_federations: Vec<String>, // "*" for all
    pub daily_limit_msats: Option<u64>,
    pub per_payment_limit_msats: Option<u64>,
    pub allowed_methods: Vec<String>,
    pub created_at: u64,
    pub last_used: Option<u64>,
    pub total_spent_msats: u64,
}
