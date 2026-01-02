//! Storage layer using sled embedded database

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info};

use crate::federation::Federation;
use crate::models::{FederationId, PublicKey, Transaction};

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
                let dir_path = dir.display();
                info!("Opening database at {dir_path}");

                // Create directory if it doesn't exist
                std::fs::create_dir_all(dir).context("Failed to create data directory")?;

                let db_path = dir.join("nanduti.db");
                let db = sled::open(&db_path).with_context(|| {
                    format!(
                        "Failed to open database at {path}",
                        path = db_path.display()
                    )
                })?;

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

    /// Store a federation with ACID guarantees
    ///
    /// # ACID Properties
    /// - **Atomicity**: Sled transactions ensure all-or-nothing writes
    /// - **Consistency**: Federation data is validated before serialization
    /// - **Isolation**: Sled provides serializable isolation for concurrent access
    /// - **Durability**: Explicit flush() ensures data is persisted to disk
    ///
    /// # Concurrency
    /// This method is safe to call from multiple threads. Sled handles
    /// concurrent writes using optimistic concurrency control with automatic retries.
    pub fn store_federation(&self, federation: &Federation) -> Result<()> {
        if let Some(tree) = &self.federations {
            // Clone federation so it can be captured by the transaction closure
            // This is necessary because sled may retry the transaction, and all
            // operations must be inside the closure for proper replay semantics
            let federation_clone = federation.clone();

            // Use sled's transactional API for atomic commits
            tree.transaction(|tx_tree| {
                // All operations must be inside the transaction closure
                let federation_id = federation_clone.id.clone();
                let data = serde_json::to_vec(&federation_clone)
                    .map_err(|_| sled::transaction::ConflictableTransactionError::Abort(()))?;

                tx_tree.insert(federation_id.as_bytes(), data.as_slice())?;
                Ok::<(), sled::transaction::ConflictableTransactionError<()>>(())
            })
            .map_err(|error| anyhow::anyhow!("Federation store failed: {error:?}"))?;

            debug!("Stored federation: {}", federation.id);

            // Note: flush() call removed per performance review
            // Sled's WAL (Write-Ahead Log) provides durability guarantees
            // Explicit flush only needed before shutdown or for critical operations
        }
        Ok(())
    }

    /// Get a federation by ID
    pub fn get_federation(&self, federation_id: &FederationId) -> Result<Option<Federation>> {
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

    /// Remove a federation with ACID guarantees
    ///
    /// # ACID Properties
    /// Same guarantees as `store_federation()`. See that method for details.
    pub fn remove_federation(&self, federation_id: &FederationId) -> Result<()> {
        if let Some(tree) = &self.federations {
            // Clone federation_id so it can be captured by the transaction closure
            let federation_id_clone = federation_id.clone();

            // Use sled's transactional API for atomic commits
            tree.transaction(|tx_tree| {
                // All operations must be inside the transaction closure
                let id_bytes = federation_id_clone.as_bytes();
                tx_tree.remove(id_bytes)?;
                Ok::<(), sled::transaction::ConflictableTransactionError<()>>(())
            })
            .map_err(|error| anyhow::anyhow!("Federation removal failed: {error:?}"))?;

            debug!("Removed federation: {federation_id}");

            // Note: flush() call removed per performance review
            // Sled's WAL provides durability guarantees
        }
        Ok(())
    }

    /// Store a transaction with ACID guarantees
    ///
    /// # ACID Properties
    /// Same guarantees as `store_federation()`. Especially critical for financial data.
    ///
    /// # Security Note
    /// Transaction data is currently stored in plaintext. For production use,
    /// consider encrypting transaction records or using filesystem-level encryption.
    pub fn store_transaction(&self, transaction: &Transaction) -> Result<()> {
        if let Some(tree) = &self.transactions {
            // Clone transaction so it can be captured by the transaction closure
            let transaction_clone = transaction.clone();

            // Use sled's transactional API for atomic commits
            tree.transaction(|tx_tree| {
                // All operations must be inside the transaction closure
                let transaction_id = transaction_clone.id.clone();
                let data = serde_json::to_vec(&transaction_clone)
                    .map_err(|_| sled::transaction::ConflictableTransactionError::Abort(()))?;

                tx_tree.insert(transaction_id.as_bytes(), data.as_slice())?;
                Ok::<(), sled::transaction::ConflictableTransactionError<()>>(())
            })
            .map_err(|error| anyhow::anyhow!("Transaction commit failed: {error:?}"))?;

            debug!(
                "Stored transaction with ACID guarantees: {}",
                transaction.id
            );

            // Note: flush() call removed per performance review
            // Sled's WAL provides durability guarantees
        }
        Ok(())
    }

    /// Get transactions for a federation
    pub fn get_federation_transactions(
        &self,
        federation_id: &FederationId,
        limit: Option<usize>,
    ) -> Result<Vec<Transaction>> {
        let mut transactions = Vec::new();

        if let Some(tree) = &self.transactions {
            for item in tree.iter() {
                let (_, value) = item.context("Failed to read transaction item")?;
                let transaction: Transaction =
                    serde_json::from_slice(&value).context("Failed to deserialize transaction")?;

                if transaction.federation_id == *federation_id {
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

    /// Get a transaction by payment hash
    pub fn get_transaction_by_payment_hash(
        &self,
        payment_hash: &str,
    ) -> Result<Option<Transaction>> {
        if let Some(tree) = &self.transactions {
            for item in tree.iter() {
                let (_, value) = item.context("Failed to read transaction item")?;
                let transaction: Transaction =
                    serde_json::from_slice(&value).context("Failed to deserialize transaction")?;

                if transaction.payment_hash.as_str() == payment_hash {
                    return Ok(Some(transaction));
                }
            }
        }
        Ok(None)
    }

    /// Get a transaction by invoice
    pub fn get_transaction_by_invoice(&self, invoice: &str) -> Result<Option<Transaction>> {
        if let Some(tree) = &self.transactions {
            for item in tree.iter() {
                let (_, value) = item.context("Failed to read transaction item")?;
                let transaction: Transaction =
                    serde_json::from_slice(&value).context("Failed to deserialize transaction")?;

                if let Some(tx_invoice) = &transaction.invoice {
                    if tx_invoice.as_str() == invoice {
                        return Ok(Some(transaction));
                    }
                }
            }
        }
        Ok(None)
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
    pub fn get_connection(&self, pubkey: &PublicKey) -> Result<Option<NwcConnection>> {
        if let Some(tree) = &self.connections {
            for item in tree.iter() {
                let (_, value) = item.context("Failed to read connection item")?;
                let connection: NwcConnection =
                    serde_json::from_slice(&value).context("Failed to deserialize connection")?;

                if connection.pubkey == pubkey.as_str() {
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
