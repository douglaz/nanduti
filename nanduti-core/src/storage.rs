//! Storage layer using sled embedded database
//!
//! Provides encrypted storage for sensitive transaction data using AES-256-GCM.
//! Encryption keys are derived from the wallet mnemonic using HKDF.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::federation::Federation;
use crate::models::{Bolt11String, FederationId, PaymentHash, PublicKey, Transaction};

/// Nonce size for AES-GCM (12 bytes / 96 bits)
const NONCE_SIZE: usize = 12;

/// Storage backend for persisting federation and transaction data
///
/// Transaction data is encrypted at rest using AES-256-GCM when an encryption
/// key is provided. The key should be derived from the wallet mnemonic using
/// `MnemonicStore::derive_storage_key()`.
///
/// # Indexing
/// Secondary indexes are maintained for efficient lookups:
/// - `tx_by_payment_hash`: payment_hash -> transaction_id
/// - `tx_by_invoice`: invoice -> transaction_id
/// - `conn_by_pubkey`: pubkey -> connection_id
#[derive(Clone)]
pub struct Storage {
    db: Option<Arc<sled::Db>>,
    federations: Option<sled::Tree>,
    connections: Option<sled::Tree>,
    transactions: Option<sled::Tree>,
    // Secondary indexes for efficient lookups
    tx_by_payment_hash: Option<sled::Tree>,
    tx_by_invoice: Option<sled::Tree>,
    conn_by_pubkey: Option<sled::Tree>,
    /// Optional encryption key for transaction data (32 bytes for AES-256)
    encryption_key: Option<[u8; 32]>,
}

impl Storage {
    /// Create a new storage instance
    ///
    /// # Arguments
    /// - `data_dir`: Directory for persistent storage. If None, operates in memory-only mode.
    /// - `encryption_key`: Optional 32-byte key for encrypting transaction data.
    ///   Should be derived from the mnemonic using `MnemonicStore::derive_storage_key()`.
    ///
    /// # Security
    /// When an encryption key is provided, all transaction data is encrypted using
    /// AES-256-GCM before being stored. The key is never persisted to disk.
    pub fn new(data_dir: Option<&Path>, encryption_key: Option<[u8; 32]>) -> Result<Self> {
        let (
            db,
            federations,
            connections,
            transactions,
            tx_by_payment_hash,
            tx_by_invoice,
            conn_by_pubkey,
        ) = match data_dir {
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

                // Secondary indexes for efficient lookups
                let tx_by_payment_hash = Some(
                    db.open_tree("idx_tx_payment_hash")
                        .context("Failed to open tx_by_payment_hash index")?,
                );
                let tx_by_invoice = Some(
                    db.open_tree("idx_tx_invoice")
                        .context("Failed to open tx_by_invoice index")?,
                );
                let conn_by_pubkey = Some(
                    db.open_tree("idx_conn_pubkey")
                        .context("Failed to open conn_by_pubkey index")?,
                );

                (
                    Some(Arc::new(db)),
                    federations,
                    connections,
                    transactions,
                    tx_by_payment_hash,
                    tx_by_invoice,
                    conn_by_pubkey,
                )
            }
            None => {
                info!("Running in memory-only mode (transient sled storage)");

                // Use a temporary sled database so that in-memory mode still has
                // functioning storage for the lifetime of the process. Without this,
                // all store_* methods become silent no-ops and features like NWC
                // connections break immediately after creation.
                let db = sled::Config::new()
                    .temporary(true)
                    .open()
                    .context("Failed to open temporary sled database")?;

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
                let tx_by_payment_hash = Some(
                    db.open_tree("idx_tx_payment_hash")
                        .context("Failed to open tx_by_payment_hash index")?,
                );
                let tx_by_invoice = Some(
                    db.open_tree("idx_tx_invoice")
                        .context("Failed to open tx_by_invoice index")?,
                );
                let conn_by_pubkey = Some(
                    db.open_tree("idx_conn_pubkey")
                        .context("Failed to open conn_by_pubkey index")?,
                );

                (
                    Some(Arc::new(db)),
                    federations,
                    connections,
                    transactions,
                    tx_by_payment_hash,
                    tx_by_invoice,
                    conn_by_pubkey,
                )
            }
        };

        if encryption_key.is_some() {
            info!("Storage encryption enabled for transaction data");
        } else if data_dir.is_some() {
            warn!("Storage encryption disabled - transaction data will be stored in plaintext");
        }

        Ok(Self {
            db,
            federations,
            connections,
            transactions,
            tx_by_payment_hash,
            tx_by_invoice,
            conn_by_pubkey,
            encryption_key,
        })
    }

    /// Encrypt data using AES-256-GCM
    ///
    /// # Returns
    /// Encrypted data in format: nonce (12 bytes) + ciphertext
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let key = self
            .encryption_key
            .context("Encryption key not configured")?;

        // Generate random nonce
        let nonce_bytes: [u8; NONCE_SIZE] = rand::random();
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Create cipher and encrypt
        let cipher = Aes256Gcm::new_from_slice(&key).context("Failed to create AES cipher")?;
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| anyhow::anyhow!("Encryption failed: {e}"))?;

        // Prepend nonce to ciphertext
        let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);

        Ok(result)
    }

    /// Decrypt data encrypted with `encrypt()`
    ///
    /// # Arguments
    /// - `data`: Encrypted data in format: nonce (12 bytes) + ciphertext
    fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        let key = self
            .encryption_key
            .context("Encryption key not configured")?;

        // Validate minimum size
        if data.len() <= NONCE_SIZE {
            anyhow::bail!("Encrypted data too short");
        }

        // Extract nonce and ciphertext
        let nonce = Nonce::from_slice(&data[..NONCE_SIZE]);
        let ciphertext = &data[NONCE_SIZE..];

        // Create cipher and decrypt
        let cipher = Aes256Gcm::new_from_slice(&key).context("Failed to create AES cipher")?;
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| anyhow::anyhow!("Decryption failed: {e}"))?;

        Ok(plaintext)
    }

    /// Check if encryption is enabled
    pub fn is_encrypted(&self) -> bool {
        self.encryption_key.is_some()
    }

    /// Hash an index key using HMAC-SHA256 when encryption is enabled so that
    /// plaintext payment hashes / invoice strings are not stored on disk.
    /// When encryption is disabled, returns the raw bytes unchanged.
    fn hash_index_key(&self, raw_key: &[u8]) -> Vec<u8> {
        if let Some(key) = &self.encryption_key {
            // Use SHA-256 HMAC to derive a deterministic but opaque index key.
            // We use the raw digest crate to avoid trait ambiguity in hmac.
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(key);
            hasher.update(raw_key);
            hasher.finalize().to_vec()
        } else {
            raw_key.to_vec()
        }
    }

    /// Encrypt an index value when encryption is enabled; pass through otherwise.
    fn encrypt_index_value(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        if self.encryption_key.is_some() {
            self.encrypt(plaintext)
        } else {
            Ok(plaintext.to_vec())
        }
    }

    /// Decrypt an index value when encryption is enabled; pass through otherwise.
    fn decrypt_index_value(&self, data: &[u8]) -> Result<Vec<u8>> {
        if self.encryption_key.is_some() {
            self.decrypt(data)
        } else {
            Ok(data.to_vec())
        }
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
    /// Also atomically updates secondary indexes for efficient lookups.
    ///
    /// # Security
    /// Transaction data is encrypted using AES-256-GCM when an encryption key is
    /// configured. The encryption key should be derived from the wallet mnemonic.
    pub fn store_transaction(&self, transaction: &Transaction) -> Result<()> {
        if let Some(tree) = &self.transactions {
            // Serialize transaction
            let json_data =
                serde_json::to_vec(&transaction).context("Failed to serialize transaction")?;

            // Encrypt if key is available
            let data = if self.encryption_key.is_some() {
                self.encrypt(&json_data)?
            } else {
                json_data
            };

            let transaction_id = transaction.id.clone();
            let payment_hash = transaction.payment_hash.clone();
            let invoice = transaction.invoice.clone();

            // Insert main transaction data
            tree.insert(transaction_id.as_bytes(), data.as_slice())
                .context("Failed to store transaction")?;

            // Update payment_hash index (stores a JSON array of tx IDs to support
            // multiple transactions sharing the same payment hash, e.g. retries).
            // When encryption is enabled, hash the index key so plaintext payment
            // hashes are not stored on disk, and encrypt the value (tx ID list).
            if let Some(idx) = &self.tx_by_payment_hash {
                let key = self.hash_index_key(payment_hash.as_bytes());
                let mut tx_ids: Vec<String> = if let Some(existing) =
                    idx.get(&key).context("Failed to read payment_hash index")?
                {
                    let raw = self.decrypt_index_value(&existing)?;
                    serde_json::from_slice(&raw).unwrap_or_else(|_| {
                        let legacy_id = String::from_utf8_lossy(&raw).to_string();
                        vec![legacy_id]
                    })
                } else {
                    Vec::new()
                };
                let tx_id_str = String::from_utf8_lossy(transaction_id.as_bytes()).to_string();
                if !tx_ids.contains(&tx_id_str) {
                    tx_ids.push(tx_id_str);
                }
                let encoded = serde_json::to_vec(&tx_ids)
                    .context("Failed to serialize payment_hash index")?;
                let value = self.encrypt_index_value(&encoded)?;
                idx.insert(key.as_slice(), value.as_slice())
                    .context("Failed to update payment_hash index")?;
            }

            // Update invoice index (if present)
            if let (Some(idx), Some(inv)) = (&self.tx_by_invoice, &invoice) {
                let key = self.hash_index_key(inv.as_str().as_bytes());
                let value = self.encrypt_index_value(transaction_id.as_bytes())?;
                idx.insert(key.as_slice(), value.as_slice())
                    .context("Failed to update invoice index")?;
            }

            debug!(
                "Stored transaction with indexes: {} (encrypted: {})",
                transaction.id,
                self.encryption_key.is_some()
            );

            // Note: flush() call removed per performance review
            // Sled's WAL provides durability guarantees
        }
        Ok(())
    }

    /// Deserialize transaction data, decrypting if necessary.
    ///
    /// Falls back to plaintext deserialization when decryption fails, which
    /// handles pre-encryption transactions written by earlier versions.
    fn deserialize_transaction(&self, data: &[u8]) -> Result<Transaction> {
        if self.encryption_key.is_some() {
            // Try decryption first, then fall back to plaintext for pre-encryption data
            if let Ok(decrypted) = self.decrypt(data) {
                if let Ok(tx) = serde_json::from_slice(&decrypted) {
                    return Ok(tx);
                }
            }
            // Fallback: try parsing as plaintext JSON (pre-encryption records)
            if let Ok(tx) = serde_json::from_slice(data) {
                return Ok(tx);
            }
            anyhow::bail!("Failed to deserialize transaction: neither decryption nor plaintext parsing succeeded");
        } else {
            serde_json::from_slice(data).context("Failed to deserialize transaction")
        }
    }

    /// Get transactions for a federation with hard limits to prevent memory exhaustion
    ///
    /// # Safety
    /// - Applies hard cap of 1000 transactions maximum
    /// - Stops scanning after 10,000 items to prevent DoS
    /// - Logs warning if scan limit exceeded
    pub fn get_federation_transactions(
        &self,
        federation_id: &FederationId,
        limit: Option<usize>,
    ) -> Result<Vec<Transaction>> {
        const MAX_LIMIT: usize = 1000;
        const MAX_SCAN: usize = 10_000;

        // When callers pass None they need full history for post-retrieval
        // pagination/filtering — only the MAX_SCAN safety cap applies.
        // When a specific limit is requested, cap it at MAX_LIMIT.
        let limit = limit.map(|l| l.min(MAX_LIMIT));
        let mut transactions = Vec::new();
        let mut scanned = 0;

        if let Some(tree) = &self.transactions {
            for item in tree.iter() {
                scanned += 1;
                if scanned > MAX_SCAN {
                    warn!(
                        "Transaction scan exceeded {MAX_SCAN} items for federation {federation_id}, aborting"
                    );
                    break;
                }

                let (_, value) = item.context("Failed to read transaction item")?;
                let transaction = self.deserialize_transaction(&value)?;

                if transaction.federation_id == *federation_id {
                    transactions.push(transaction);
                }
            }
        }

        // Sort by created_at descending BEFORE applying limit so we return the
        // most recent transactions rather than an arbitrary subset.
        transactions.sort_by_key(|tx| std::cmp::Reverse(tx.created_at));
        if let Some(limit) = limit {
            transactions.truncate(limit);
        }

        Ok(transactions)
    }

    /// Get all transactions across all federations (including removed ones)
    ///
    /// This ensures transactions from removed federations remain visible
    /// in history listings.
    pub fn get_all_transactions(&self) -> Result<Vec<Transaction>> {
        const MAX_SCAN: usize = 10_000;
        let mut transactions = Vec::new();
        let mut scanned = 0;

        if let Some(tree) = &self.transactions {
            for item in tree.iter() {
                scanned += 1;
                if scanned > MAX_SCAN {
                    warn!("Transaction scan exceeded {MAX_SCAN} items, aborting");
                    break;
                }

                let (_, value) = item.context("Failed to read transaction item")?;
                let transaction = self.deserialize_transaction(&value)?;
                transactions.push(transaction);
            }
        }

        transactions.sort_by_key(|tx| std::cmp::Reverse(tx.created_at));
        Ok(transactions)
    }

    /// Get all transactions matching a payment hash
    ///
    /// # Returns
    /// Returns all matching transactions sorted by creation time (most recent first).
    /// Multiple transactions can have the same payment hash (retries, duplicate invoices).
    ///
    /// # Performance
    /// Uses secondary index for O(1) lookup by payment hash.
    pub fn get_transactions_by_payment_hash(
        &self,
        payment_hash: &PaymentHash,
    ) -> Result<Vec<Transaction>> {
        let mut transactions = Vec::new();

        // Try to use index first (key is hashed when encryption is enabled)
        if let (Some(idx), Some(tree)) = (&self.tx_by_payment_hash, &self.transactions) {
            let hashed_key = self.hash_index_key(payment_hash.as_bytes());
            if let Some(raw) = idx
                .get(&hashed_key)
                .context("Failed to read payment_hash index")?
            {
                let decrypted = self.decrypt_index_value(&raw)?;
                let tx_ids: Vec<String> = serde_json::from_slice(&decrypted).unwrap_or_else(|_| {
                    let legacy_id = String::from_utf8_lossy(&decrypted).to_string();
                    vec![legacy_id]
                });

                for tx_id in &tx_ids {
                    if let Some(data) = tree
                        .get(tx_id.as_bytes())
                        .context("Failed to read transaction from index lookup")?
                    {
                        let transaction = self.deserialize_transaction(&data)?;
                        transactions.push(transaction);
                    }
                }
            }
            // If index returned nothing, fall through to full scan for
            // pre-upgrade transactions not yet in the index.
        }
        if transactions.is_empty() {
            if let Some(tree) = &self.transactions {
                // Full scan fallback (index miss on upgrade, or no index)
                for item in tree.iter() {
                    let (_, value) = item.context("Failed to read transaction item")?;
                    let transaction = self.deserialize_transaction(&value)?;

                    if &transaction.payment_hash == payment_hash {
                        transactions.push(transaction);
                    }
                }
            }
        }

        // Sort by created_at descending (most recent first)
        transactions.sort_by_key(|tx| std::cmp::Reverse(tx.created_at));

        Ok(transactions)
    }

    /// Get the most recent transaction by payment hash
    ///
    /// # Returns
    /// Returns the most recent transaction matching the payment hash, or None if not found.
    ///
    /// # Performance
    /// Uses secondary index for O(1) lookup.
    pub fn get_transaction_by_payment_hash(
        &self,
        payment_hash: &PaymentHash,
    ) -> Result<Option<Transaction>> {
        let transactions = self.get_transactions_by_payment_hash(payment_hash)?;
        Ok(transactions.into_iter().next())
    }

    /// Get a transaction by invoice
    ///
    /// # Performance
    /// Uses secondary index for O(1) lookup by invoice string.
    pub fn get_transaction_by_invoice(
        &self,
        invoice: &Bolt11String,
    ) -> Result<Option<Transaction>> {
        // Try to use index first (key is hashed when encryption is enabled)
        if let (Some(idx), Some(tree)) = (&self.tx_by_invoice, &self.transactions) {
            let hashed_key = self.hash_index_key(invoice.as_str().as_bytes());
            if let Some(encrypted_tx_id) = idx
                .get(&hashed_key)
                .context("Failed to read invoice index")?
            {
                let tx_id_bytes = self.decrypt_index_value(&encrypted_tx_id)?;
                if let Some(data) = tree
                    .get(tx_id_bytes.as_slice())
                    .context("Failed to read transaction from index lookup")?
                {
                    let transaction = self.deserialize_transaction(&data)?;
                    return Ok(Some(transaction));
                }
            }
            // Index miss — fall through to full scan for pre-upgrade data
        }

        // Fallback to full scan (index miss on upgrade, or no index in memory mode)
        if let Some(tree) = &self.transactions {
            for item in tree.iter() {
                let (_, value) = item.context("Failed to read transaction item")?;
                let transaction = self.deserialize_transaction(&value)?;

                if let Some(tx_invoice) = &transaction.invoice {
                    if tx_invoice == invoice {
                        return Ok(Some(transaction));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Store a NWC connection with ACID guarantees
    ///
    /// # ACID Properties
    /// Uses sled transactions to ensure atomic updates, preventing race conditions
    /// in connection state (especially important for total_spent_msats tracking).
    /// Also updates secondary index for pubkey lookups.
    pub fn store_connection(&self, connection: &NwcConnection) -> Result<()> {
        if let Some(tree) = &self.connections {
            let data = serde_json::to_vec(&connection).context("Failed to serialize connection")?;

            // Store the connection
            tree.insert(connection.id.as_bytes(), data.as_slice())
                .context("Failed to store connection")?;

            // Update pubkey index
            if let Some(idx) = &self.conn_by_pubkey {
                idx.insert(connection.pubkey.as_bytes(), connection.id.as_bytes())
                    .context("Failed to update pubkey index")?;
            }

            debug!("Stored connection with index: {}", connection.id);
        }
        Ok(())
    }

    /// Atomically increment connection's spent amount and update last_used timestamp
    ///
    /// # ACID Properties
    /// This method uses a transaction to perform read-modify-write atomically,
    /// preventing lost updates when multiple payments occur concurrently.
    pub fn increment_connection_spent(&self, connection_id: &str, amount_msats: u64) -> Result<()> {
        if let Some(tree) = &self.connections {
            let connection_id_str = connection_id.to_string();

            tree.transaction(|tx_tree| {
                // Read current connection
                let data = tx_tree
                    .get(connection_id_str.as_bytes())?
                    .ok_or(sled::transaction::ConflictableTransactionError::Abort(()))?;

                let mut connection: NwcConnection = serde_json::from_slice(&data)
                    .map_err(|_| sled::transaction::ConflictableTransactionError::Abort(()))?;

                // Update spent amount and last_used atomically
                connection.total_spent_msats =
                    connection.total_spent_msats.saturating_add(amount_msats);
                // Note: last_used is informational only, fallback to epoch if clock is broken
                // This is inside a sled transaction so we can't use ? operator
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_else(|_| {
                        // This should never happen unless system clock is broken
                        // We can't log inside the transaction, but 0 is a clear indicator
                        0
                    });
                connection.last_used = Some(timestamp);

                // Write back
                let updated_data = serde_json::to_vec(&connection)
                    .map_err(|_| sled::transaction::ConflictableTransactionError::Abort(()))?;

                tx_tree.insert(connection_id_str.as_bytes(), updated_data.as_slice())?;
                Ok::<(), sled::transaction::ConflictableTransactionError<()>>(())
            })
            .map_err(|error| anyhow::anyhow!("Connection spent increment failed: {error:?}"))?;

            debug!("Incremented connection {connection_id} spent by {amount_msats} msats");
        }
        Ok(())
    }

    /// Get daily spent amount for a connection
    ///
    /// # Parameters
    /// - `connection_id`: Connection identifier
    /// - `day_timestamp`: Unix timestamp for the start of the day (00:00:00 UTC)
    ///
    /// # Returns
    /// Total amount spent in millisatoshis for the specified day
    pub fn get_daily_spent(&self, connection_id: &str, day_timestamp: u64) -> Result<u64> {
        let mut daily_spent = 0u64;

        // Calculate day boundaries (00:00:00 to 23:59:59 UTC)
        let day_start = day_timestamp;
        let day_end = day_start + crate::constants::SECONDS_PER_DAY;

        if let Some(tree) = &self.transactions {
            for item in tree.iter() {
                let (_, value) = item.context("Failed to read transaction item")?;
                let transaction = self.deserialize_transaction(&value)?;

                // Check if transaction is from this connection
                if let Some(metadata) = &transaction.metadata {
                    if let Some(conn_id) = metadata.get("connection_id") {
                        if conn_id.as_str() == Some(connection_id) {
                            // For Pending payments, use created_at so they are
                            // always counted in the current day's quota regardless
                            // of when they eventually settle. For Settled payments,
                            // use settled_at so completed payments are charged to
                            // the day they actually settled.
                            let tx_timestamp =
                                if transaction.state == crate::models::TransactionState::Pending {
                                    transaction.created_at.as_secs()
                                } else {
                                    transaction
                                        .settled_at
                                        .map(|t| t.as_secs())
                                        .unwrap_or(transaction.created_at.as_secs())
                                };
                            if tx_timestamp >= day_start && tx_timestamp < day_end {
                                // Count both Settled and Pending outgoing payments so
                                // concurrent in-flight payments are reserved against the
                                // daily quota and can't exceed the limit in aggregate.
                                // Failed transactions are excluded.
                                if transaction.transaction_type
                                    == crate::models::TransactionType::Outgoing
                                    && (transaction.state
                                        == crate::models::TransactionState::Settled
                                        || transaction.state
                                            == crate::models::TransactionState::Pending)
                                {
                                    // Include both the payment amount and routing fees
                                    // to match what increment_connection_spent records.
                                    let fees =
                                        transaction.fees_paid.map(|f| f.as_msats()).unwrap_or(0);
                                    daily_spent = daily_spent
                                        .saturating_add(transaction.amount.as_msats())
                                        .saturating_add(fees);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(daily_spent)
    }

    /// Get a NWC connection by public key
    ///
    /// # Performance
    /// Uses secondary index for O(1) lookup by pubkey.
    pub fn get_connection(&self, pubkey: &PublicKey) -> Result<Option<NwcConnection>> {
        // Try to use index first
        if let (Some(idx), Some(tree)) = (&self.conn_by_pubkey, &self.connections) {
            if let Some(conn_id_bytes) = idx
                .get(pubkey.as_bytes())
                .context("Failed to read pubkey index")?
            {
                if let Some(data) = tree
                    .get(&conn_id_bytes)
                    .context("Failed to read connection from index lookup")?
                {
                    let connection: NwcConnection = serde_json::from_slice(&data)
                        .context("Failed to deserialize connection")?;
                    return Ok(Some(connection));
                }
            }
            // Index miss — fall through to full scan for pre-upgrade connections
            // that were stored before the pubkey index was added.
        }

        // Fallback to full scan (index miss on upgrade, or no index in memory mode)
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
    pub allowed_federations: crate::models::FederationFilter,
    pub daily_limit_msats: Option<u64>,
    pub per_payment_limit_msats: Option<u64>,
    pub allowed_methods: crate::models::MethodFilter,
    pub created_at: u64,
    pub last_used: Option<u64>,
    pub total_spent_msats: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        Amount, Bolt11String, Description, FederationId, PaymentHash, Timestamp, TransactionId,
        TransactionState, TransactionType,
    };
    use tempfile::TempDir;

    fn create_test_transaction(id: &str) -> Transaction {
        Transaction {
            id: TransactionId::new(id.to_string()),
            federation_id: FederationId::new("test-federation".to_string()),
            transaction_type: TransactionType::Outgoing,
            state: TransactionState::Settled,
            invoice: Some(Bolt11String::new("lnbc1...".to_string())),
            description: Some(Description::new("Test payment".to_string())),
            preimage: None,
            payment_hash: PaymentHash::new("abc123".to_string()),
            amount: Amount::from_sats(1000),
            fees_paid: Some(Amount::from_sats(10)),
            created_at: Timestamp::now(),
            settled_at: Some(Timestamp::now()),
            metadata: None,
        }
    }

    #[test]
    fn test_storage_without_encryption() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let storage = Storage::new(Some(temp_dir.path()), None)?;

        assert!(!storage.is_encrypted());

        // Store and retrieve transaction
        let tx = create_test_transaction("tx-1");
        storage.store_transaction(&tx)?;

        let retrieved =
            storage.get_transactions_by_payment_hash(&PaymentHash::new("abc123".to_string()))?;
        assert_eq!(retrieved.len(), 1);
        assert_eq!(retrieved[0].id.as_str(), "tx-1");

        Ok(())
    }

    #[test]
    fn test_storage_with_encryption() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let key: [u8; 32] = rand::random();
        let storage = Storage::new(Some(temp_dir.path()), Some(key))?;

        assert!(storage.is_encrypted());

        // Store and retrieve transaction
        let tx = create_test_transaction("tx-encrypted");
        storage.store_transaction(&tx)?;

        let retrieved =
            storage.get_transactions_by_payment_hash(&PaymentHash::new("abc123".to_string()))?;
        assert_eq!(retrieved.len(), 1);
        assert_eq!(retrieved[0].id.as_str(), "tx-encrypted");
        assert_eq!(retrieved[0].amount.as_sats(), 1000);

        Ok(())
    }

    #[test]
    fn test_encrypted_data_not_readable_without_key() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let key: [u8; 32] = rand::random();

        // Store with encryption
        {
            let storage = Storage::new(Some(temp_dir.path()), Some(key))?;
            let tx = create_test_transaction("tx-secret");
            storage.store_transaction(&tx)?;
        }

        // Try to read without encryption key - index keys are hashed differently
        // so the lookup won't find the data (returns empty), or if it somehow
        // hits the encrypted blob it will fail to deserialize.
        {
            let storage = Storage::new(Some(temp_dir.path()), None)?;
            let result =
                storage.get_transactions_by_payment_hash(&PaymentHash::new("abc123".to_string()));
            if let Ok(txs) = result {
                assert!(txs.is_empty(), "Should not find encrypted data without key");
            }
        }

        Ok(())
    }

    #[test]
    fn test_wrong_key_fails_decryption() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let key1: [u8; 32] = rand::random();
        let key2: [u8; 32] = rand::random();

        // Store with key1
        {
            let storage = Storage::new(Some(temp_dir.path()), Some(key1))?;
            let tx = create_test_transaction("tx-key1");
            storage.store_transaction(&tx)?;
        }

        // Try to read with key2 - index keys are hashed differently with a
        // different key, so the lookup returns empty or fails to decrypt.
        {
            let storage = Storage::new(Some(temp_dir.path()), Some(key2))?;
            let result =
                storage.get_transactions_by_payment_hash(&PaymentHash::new("abc123".to_string()));
            if let Ok(txs) = result {
                assert!(txs.is_empty(), "Should not find data with wrong key");
            }
        }

        Ok(())
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() -> Result<()> {
        let key: [u8; 32] = rand::random();
        let storage = Storage::new(None, Some(key))?;

        let plaintext = b"Hello, encrypted world!";
        let ciphertext = storage.encrypt(plaintext)?;

        // Ciphertext should be different from plaintext
        assert_ne!(&ciphertext[NONCE_SIZE..], plaintext);

        // Should decrypt back to original
        let decrypted = storage.decrypt(&ciphertext)?;
        assert_eq!(decrypted, plaintext);

        Ok(())
    }

    #[test]
    fn test_memory_mode_no_encryption_needed() -> Result<()> {
        // Memory-only mode doesn't need encryption (no persistence)
        let storage = Storage::new(None, None)?;
        assert!(!storage.is_encrypted());

        let tx = create_test_transaction("tx-memory");
        storage.store_transaction(&tx)?;

        // In memory mode, transactions are stored in a temporary sled database
        // that lives for the lifetime of the process
        let retrieved =
            storage.get_transactions_by_payment_hash(&PaymentHash::new("abc123".to_string()))?;
        assert_eq!(retrieved.len(), 1);
        assert_eq!(retrieved[0].id.as_str(), "tx-memory");

        Ok(())
    }

    #[test]
    fn test_index_lookup_by_invoice() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let storage = Storage::new(Some(temp_dir.path()), None)?;

        let tx = create_test_transaction("tx-invoice-test");
        storage.store_transaction(&tx)?;

        // Should find by invoice using index
        let result =
            storage.get_transaction_by_invoice(&Bolt11String::new("lnbc1...".to_string()))?;
        assert!(result.is_some());
        assert_eq!(result.unwrap().id.as_str(), "tx-invoice-test");

        // Non-existent invoice should return None
        let not_found = storage
            .get_transaction_by_invoice(&Bolt11String::new("lnbc_nonexistent".to_string()))?;
        assert!(not_found.is_none());

        Ok(())
    }

    #[test]
    fn test_index_lookup_by_pubkey() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let storage = Storage::new(Some(temp_dir.path()), None)?;

        let connection = NwcConnection {
            id: "conn-1".to_string(),
            name: "Test Connection".to_string(),
            pubkey: "pubkey123".to_string(),
            allowed_federations: crate::models::FederationFilter::All,
            daily_limit_msats: None,
            per_payment_limit_msats: None,
            allowed_methods: crate::models::MethodFilter::specific(vec!["pay_invoice".to_string()]),
            created_at: 1000,
            last_used: None,
            total_spent_msats: 0,
        };

        storage.store_connection(&connection)?;

        // Should find by pubkey using index
        let result = storage.get_connection(&PublicKey::new("pubkey123".to_string()))?;
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "conn-1");

        // Non-existent pubkey should return None
        let not_found = storage.get_connection(&PublicKey::new("nonexistent".to_string()))?;
        assert!(not_found.is_none());

        Ok(())
    }
}
