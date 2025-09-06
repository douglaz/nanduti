//! Wrapper for Fedimint client operations

use anyhow::{bail, Context, Result};
use fedimint_bip39::{Bip39RootSecretStrategy, Mnemonic};
use fedimint_client::module_init::ClientModuleInitRegistry;
use fedimint_client::secret::RootSecretStrategy;
use fedimint_client::{Client, ClientHandleArc, RootSecret};
use fedimint_core::config::FederationId;
use fedimint_core::db::Database;
use fedimint_core::invite_code::InviteCode;
use fedimint_core::Amount as FedimintAmount;
use fedimint_ln_client::LightningClientModule;
use fedimint_ln_common::{LightningGateway, LightningGatewayAnnouncement};
use fedimint_meta_client::{common::MetaKey, MetaClientInit, MetaClientModule};
use fedimint_mint_client::{MintClientInit, MintClientModule};
use fedimint_wallet_client::WalletClientInit;
use lightning_invoice::Bolt11Invoice;
use rand::rngs::OsRng;
use rand::seq::IteratorRandom;
use std::convert::TryFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;

use crate::lightning::PaymentResult;
use crate::mnemonic_store::MnemonicStore;
use crate::models::{
    Amount, Bolt11String, Description, Expiry, GatewayVettingStatus, Invoice, PaymentHash,
    Preimage, PublicKey,
};

/// Wrapper around the actual Fedimint client
/// This abstracts the Fedimint client API for easier testing and maintenance
#[derive(Debug, Clone)]
pub struct FedimintClientWrapper {
    client: ClientHandleArc,
    federation_id: FederationId,
    federation_name: String,
    #[allow(dead_code)]
    db_path: PathBuf,
}

impl FedimintClientWrapper {
    /// Create a new Fedimint client from invite code
    pub async fn new(invite: &InviteCode, data_dir: Option<&Path>) -> Result<Self> {
        info!("Initializing Fedimint client from invite code");

        let federation_id = invite.federation_id();

        // Create database path
        let base_dir = data_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".nanduti"));
        let db_path = base_dir.join(format!("federation_{federation_id}"));
        std::fs::create_dir_all(&db_path)?;

        // Open database
        let db_file = db_path.join("client.db");
        let locked_db = fedimint_cursed_redb::MemAndRedb::new(&db_file)
            .await
            .context("Failed to open database")?;
        let db = Database::new(locked_db, Default::default());

        // Generate or load mnemonic first
        let mnemonic = Self::load_or_generate_mnemonic(&db_path).await?;
        let root_secret = RootSecret::StandardDoubleDerive(
            Bip39RootSecretStrategy::<12>::to_root_secret(&mnemonic),
        );

        // Try to open existing client first
        {
            let mut client_builder = Client::builder(db.clone()).await?;
            client_builder.with_module_inits(Self::build_module_inits());
            client_builder.with_primary_module_kind(fedimint_mint_client::KIND);

            if let Ok(client) = client_builder.open(root_secret.clone()).await {
                info!("Opened existing client for federation {federation_id}");

                // Get federation name from config
                let config = client.config().await;
                let federation_name = config
                    .global
                    .meta
                    .get("federation_name")
                    .cloned()
                    .unwrap_or_else(|| "Unknown Federation".to_string());

                return Ok(Self {
                    client: Arc::new(client),
                    federation_id,
                    federation_name,
                    db_path,
                });
            }
        }

        // Join new federation if client doesn't exist
        info!("Joining new federation {federation_id}");

        // Create a new client builder for joining
        let mut client_builder = Client::builder(db.clone()).await?;
        client_builder.with_module_inits(Self::build_module_inits());
        client_builder.with_primary_module_kind(fedimint_mint_client::KIND);

        // Preview the federation
        let client_config = client_builder
            .preview(invite)
            .await
            .context("Failed to preview federation")?;

        // Get federation name from preview
        let federation_name = client_config
            .config()
            .global
            .meta
            .get("federation_name")
            .cloned()
            .unwrap_or_else(|| "Unknown Federation".to_string());

        // Join the federation
        let root_secret = RootSecret::StandardDoubleDerive(
            Bip39RootSecretStrategy::<12>::to_root_secret(&mnemonic),
        );

        let client = client_config
            .join(root_secret)
            .await
            .map(Arc::new)
            .context("Failed to join federation")?;

        info!("Successfully joined federation: {federation_name}");

        Ok(Self {
            client,
            federation_id,
            federation_name,
            db_path,
        })
    }

    /// Build module initializers
    fn build_module_inits() -> ClientModuleInitRegistry {
        let mut registry = ClientModuleInitRegistry::new();
        registry.attach(MintClientInit);
        registry.attach(fedimint_ln_client::LightningClientInit::default());
        registry.attach(WalletClientInit::default());
        registry.attach(MetaClientInit);
        // Note: fedimint_lnv2_client doesn't exist in 0.8.1
        // registry.attach(fedimint_lnv2_client::LightningClientInit::default());
        registry
    }

    /// Load or generate mnemonic
    async fn load_or_generate_mnemonic(db_path: &Path) -> Result<Mnemonic> {
        // Try to load existing mnemonic
        if let Some(mnemonic) = MnemonicStore::load_mnemonic(db_path, None).await? {
            return Ok(mnemonic);
        }

        // Generate new mnemonic if none exists
        let entropy = rand::random::<[u8; 16]>();
        let mnemonic = Mnemonic::from_entropy(&entropy)?;

        // Store the new mnemonic
        MnemonicStore::store_mnemonic(db_path, &mnemonic, None).await?;

        Ok(mnemonic)
    }

    /// Get current balance
    pub async fn get_balance(&self) -> Result<Amount> {
        // Get the mint module
        let mint_module = self
            .client
            .get_first_module::<MintClientModule>()
            .context("Mint module not available")?;

        // Get note counts by denomination from the database
        let summary = mint_module
            .get_note_counts_by_denomination(
                &mut self
                    .client
                    .db()
                    .begin_transaction_nc()
                    .await
                    .to_ref_with_prefix_module_id(1)
                    .0,
            )
            .await;

        let total_msats = summary.total_amount().msats;

        Ok(Amount::from_msats(total_msats))
    }

    /// Pay a lightning invoice
    pub async fn pay_invoice(&self, invoice: &Invoice) -> Result<PaymentResult> {
        // Convert to Bolt11Invoice
        let bolt11 = Bolt11Invoice::try_from(invoice).context("Failed to parse BOLT11 invoice")?;

        // Get the lightning module
        let ln_module = self
            .client
            .get_first_module::<LightningClientModule>()
            .context("Lightning module not available")?;

        info!("Paying invoice via federation {}", self.federation_name);

        // Pay the invoice and get the operation
        let outgoing_payment = ln_module
            .pay_bolt11_invoice(None, bolt11.clone(), ())
            .await
            .context("Failed to pay invoice")?;

        // Wait for the payment to complete
        let payment_result = ln_module
            .wait_for_ln_payment(
                outgoing_payment.payment_type,
                outgoing_payment.contract_id,
                false, // Don't return early
            )
            .await?
            .context("Payment did not complete")?;

        // Extract payment details from the result (JSON)
        let preimage_str = payment_result
            .get("preimage")
            .and_then(|v| v.as_str())
            .context("Payment succeeded but no preimage available")?;

        let preimage_hex = preimage_str.to_string();
        let payment_hash = hex::encode(bolt11.payment_hash().as_ref() as &[u8]);

        Ok(PaymentResult {
            preimage: Preimage::new(preimage_hex),
            fees_paid: Some(Amount::from_msats(outgoing_payment.fee.msats)),
            payment_hash: PaymentHash::new(payment_hash),
            amount_paid: invoice.amount.unwrap_or(Amount::from_msats(
                bolt11.amount_milli_satoshis().unwrap_or(0),
            )),
        })
    }

    /// Fetch available gateways from the federation
    pub async fn fetch_gateways(&self) -> Result<Vec<LightningGatewayAnnouncement>> {
        let ln_module = self
            .client
            .get_first_module::<LightningClientModule>()
            .context("Lightning module not available")?;

        // Update gateway cache to get latest gateways
        ln_module
            .update_gateway_cache()
            .await
            .context("Failed to update gateway cache")?;

        // List available gateways
        let gateways = ln_module.list_gateways().await;

        info!(
            "Found {} gateways in federation {}",
            gateways.len(),
            self.federation_name
        );
        Ok(gateways)
    }

    /// Fetch available gateways with vetted status
    pub async fn fetch_gateways_with_vetted_status(
        &self,
    ) -> Result<Vec<(LightningGatewayAnnouncement, GatewayVettingStatus)>> {
        let gateways = self.fetch_gateways().await?;
        let vetted_policy = self.fetch_vetted_gateways().await?;

        let gateways_with_status: Vec<(LightningGatewayAnnouncement, GatewayVettingStatus)> =
            match vetted_policy {
                Some(vetted_ids) => {
                    // Vetting policy exists - classify gateways as Vetted or NotVetted
                    gateways
                        .into_iter()
                        .map(|g| {
                            let status = if vetted_ids.contains(&g.info.gateway_id.to_string()) {
                                GatewayVettingStatus::Vetted
                            } else {
                                GatewayVettingStatus::NotVetted
                            };
                            (g, status)
                        })
                        .collect()
                }
                None => {
                    // No vetting policy - all gateways are Unknown (acceptable)
                    gateways
                        .into_iter()
                        .map(|g| (g, GatewayVettingStatus::Unknown))
                        .collect()
                }
            };

        let vetted_count = gateways_with_status
            .iter()
            .filter(|(_, s)| *s == GatewayVettingStatus::Vetted)
            .count();
        let not_vetted_count = gateways_with_status
            .iter()
            .filter(|(_, s)| *s == GatewayVettingStatus::NotVetted)
            .count();
        let unknown_count = gateways_with_status
            .iter()
            .filter(|(_, s)| *s == GatewayVettingStatus::Unknown)
            .count();

        info!(
            "Found {} gateways in federation {}: {} vetted, {} not-vetted, {} unrestricted",
            gateways_with_status.len(),
            self.federation_name,
            vetted_count,
            not_vetted_count,
            unknown_count
        );

        Ok(gateways_with_status)
    }

    /// Select an appropriate gateway for operations
    pub async fn select_gateway(&self) -> Result<Option<LightningGateway>> {
        let gateways_with_status = self.fetch_gateways_with_vetted_status().await?;

        if gateways_with_status.is_empty() {
            info!(
                "No gateways available in federation {}",
                self.federation_name
            );
            return Ok(None);
        }

        // Separate gateways by vetting status
        let mut vetted = Vec::new();
        let mut unknown = Vec::new();
        let mut not_vetted = Vec::new();

        for (gateway, status) in gateways_with_status {
            match status {
                GatewayVettingStatus::Vetted => vetted.push(gateway),
                GatewayVettingStatus::Unknown => unknown.push(gateway),
                GatewayVettingStatus::NotVetted => not_vetted.push(gateway),
            }
        }

        // Selection priority: Vetted > Unknown > Never select NotVetted
        let (selected, status_name) = if !vetted.is_empty() {
            info!(
                "Selecting from {} vetted gateways in federation {}",
                vetted.len(),
                self.federation_name
            );
            (
                vetted
                    .into_iter()
                    .choose(&mut OsRng)
                    .map(|announcement| announcement.info),
                "vetted",
            )
        } else if !unknown.is_empty() {
            info!(
                "No vetted gateways available, selecting from {} unrestricted gateways in federation {} (no vetting policy)",
                unknown.len(), self.federation_name
            );
            (
                unknown
                    .into_iter()
                    .choose(&mut OsRng)
                    .map(|announcement| announcement.info),
                "unrestricted",
            )
        } else {
            info!(
                "Warning: Only {} not-vetted gateways available in federation {} - cannot select (policy forbids)",
                not_vetted.len(), self.federation_name
            );
            (None, "none")
        };

        if let Some(ref gateway) = selected {
            info!(
                "Selected {} gateway {} for federation {}",
                status_name, gateway.gateway_id, self.federation_name
            );
        }

        Ok(selected)
    }

    /// Generate a new invoice
    pub async fn make_invoice(
        &self,
        amount: Amount,
        description: String,
        expiry: Option<u64>,
    ) -> Result<Invoice> {
        // Get the lightning module
        let ln_module = self
            .client
            .get_first_module::<LightningClientModule>()
            .context("Lightning module not available")?;

        let fedimint_amount = FedimintAmount::from_msats(amount.as_msats());

        // Select a gateway for routing hints
        let gateway = self.select_gateway().await?;

        if gateway.is_none() {
            info!(
                "Warning: Creating invoice without gateway routing hints for federation {}",
                self.federation_name
            );
        }

        info!(
            "Creating invoice via federation {} for {} msats with gateway: {}",
            self.federation_name,
            amount.as_msats(),
            gateway
                .as_ref()
                .map(|g| g.gateway_id.to_string())
                .unwrap_or_else(|| "none".to_string())
        );

        // Create the invoice with proper parameters for fedimint 0.8.1
        let invoice_description = lightning_invoice::Bolt11InvoiceDescription::Direct(
            lightning_invoice::Description::new(description.clone())
                .context("Invalid invoice description")?,
        );

        let (_operation_id, invoice, _preimage) = ln_module
            .create_bolt11_invoice(
                fedimint_amount,
                invoice_description,
                Some(expiry.unwrap_or(3600)),
                (),      // extra_meta
                gateway, // gateway for routing hints
            )
            .await
            .context("Failed to create invoice")?;

        let payment_hash = hex::encode(invoice.payment_hash().as_ref() as &[u8]);

        Ok(Invoice {
            bolt11: Bolt11String::new(invoice.to_string()),
            payment_hash: PaymentHash::new(payment_hash),
            amount: Some(amount),
            description: Some(Description::new(description)),
            expiry: expiry.map(Expiry::from_secs),
            payee_pubkey: None,
            created_at: Some(invoice.timestamp()),
        })
    }

    /// Perform a keysend payment (not yet supported by Fedimint)
    pub async fn pay_keysend(
        &self,
        _pubkey: &PublicKey,
        _amount: Amount,
        _preimage: Option<Vec<u8>>,
    ) -> Result<PaymentResult> {
        // Fedimint doesn't directly support keysend yet
        // This would need to be implemented as a custom module or gateway feature
        bail!("Keysend payments are not yet supported by Fedimint")
    }

    /// Fetch vetted gateway IDs from the meta module
    /// Returns None if no vetting policy exists, Some(vec) if policy exists
    pub async fn fetch_vetted_gateways(&self) -> Result<Option<Vec<String>>> {
        // Try to get the meta module
        let meta_module = match self.client.get_first_module::<MetaClientModule>() {
            Ok(module) => module,
            Err(_) => {
                // Meta module not available, no vetting policy
                info!(
                    "Meta module not available in federation {} - no vetting policy",
                    self.federation_name
                );
                return Ok(None);
            }
        };

        // Fetch consensus value at key 0 (vetted_gateways convention)
        let meta_key = MetaKey(0);
        let consensus_value = meta_module
            .get_consensus_value(meta_key)
            .await
            .context("Failed to fetch consensus value from meta module")?;

        if let Some(consensus) = consensus_value {
            // Try to parse MetaValue as JSON
            match consensus.value.to_json() {
                Ok(json) => {
                    // Look for vetted_gateways field
                    if let Some(vetted) = json.get("vetted_gateways") {
                        if let Some(gateway_array) = vetted.as_array() {
                            let gateway_ids: Vec<String> = gateway_array
                                .iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect();

                            info!(
                                "Found vetted gateway policy with {} approved gateways in federation {} (revision {})",
                                gateway_ids.len(), self.federation_name, consensus.revision
                            );
                            return Ok(Some(gateway_ids));
                        }
                    }
                }
                Err(e) => {
                    info!(
                        "Failed to parse meta value as JSON in federation {}: {e}",
                        self.federation_name
                    );
                }
            }
        }

        // No vetted gateway policy configured
        info!(
            "No vetted gateway policy configured in federation {}",
            self.federation_name
        );
        Ok(None)
    }

    /// Get federation info
    pub async fn get_info(&self) -> Result<FederationInfo> {
        let config = self.client.config().await;

        // Get network from config
        let network = config
            .global
            .meta
            .get("network")
            .cloned()
            .unwrap_or_else(|| "bitcoin".to_string());

        // Block height is not directly available from the federation modules
        // Use a reasonable recent block height as default
        // In a production system, this could be fetched from an external Bitcoin node
        let block_height = 850000; // Recent mainnet block height

        Ok(FederationInfo {
            network,
            block_height,
            synced: true,
            federation_id: self.federation_id.to_string(),
            federation_name: self.federation_name.clone(),
        })
    }

    /// Estimate fee for a payment
    pub async fn estimate_fee(&self, amount: Amount) -> Result<Amount> {
        // Verify lightning module is available
        let _ln_module = self
            .client
            .get_first_module::<LightningClientModule>()
            .context("Lightning module not available")?;

        // Try to get gateway fee schedule
        // Gateway fees typically consist of:
        // - Base fee (fixed amount per payment)
        // - Proportional fee (percentage of payment amount)

        // Use default gateway fee structure
        // These are typical values for Lightning gateways
        // In a full implementation, we would query the gateway for actual fees
        let base_fee_msats = 1000; // 1 sat base fee
        let proportional_fee_ppm = 2500; // 0.25% (2500 parts per million)

        // Calculate total fee
        let proportional_fee = (amount.as_msats() * proportional_fee_ppm) / 1_000_000;
        let total_fee = base_fee_msats + proportional_fee;

        // Ensure minimum fee of 10 msats
        Ok(Amount::from_msats(total_fee.max(10)))
    }
}

#[derive(Debug, Clone)]
pub struct FederationInfo {
    pub network: String,
    pub block_height: u64,
    pub synced: bool,
    pub federation_id: String,
    pub federation_name: String,
}
