//! Nostr relay connection and message handling

use anyhow::{Context, Result};
use nostr_sdk::prelude::*;
use std::sync::Arc;
use tracing::{debug, info};

/// Manages connection to Nostr relays
pub struct NostrClient {
    client: Client,
    keys: Keys,
}

impl NostrClient {
    /// Create a new Nostr client
    pub async fn new(relays: Vec<String>, secret_key: Option<String>) -> Result<Self> {
        // Generate or use provided keys
        let keys = if let Some(secret) = secret_key {
            Keys::parse(&secret)?
        } else {
            Keys::generate()
        };

        let client = Client::new(keys.clone());

        // Add relays
        for relay_url in relays {
            client
                .add_relay(&relay_url)
                .await
                .with_context(|| format!("Failed to add relay {relay_url}"))?;
        }

        // Connect to relays
        client.connect().await;

        info!("Connected to {} relays", client.relays().await.len());

        Ok(Self { client, keys })
    }

    /// Get the client's public key
    pub fn public_key(&self) -> String {
        self.keys.public_key().to_hex()
    }

    /// Subscribe to NWC requests
    pub async fn subscribe_nwc_requests(&self) -> Result<()> {
        // Subscribe to kind 23194 events sent to us
        let filter = Filter::new()
            .kind(Kind::from(23194))
            .pubkey(self.keys.public_key());

        self.client.subscribe(filter, None).await?;

        debug!("Subscribed to NWC requests");
        Ok(())
    }

    /// Send NWC response
    pub async fn send_nwc_response(
        &self,
        request_id: String,
        recipient_pubkey: String,
        encrypted_content: String,
    ) -> Result<()> {
        let recipient = PublicKey::from_hex(&recipient_pubkey)?;

        // Create response event (kind 23195)
        let event_builder = EventBuilder::new(Kind::from(23195), encrypted_content).tags(vec![
            Tag::public_key(recipient),
            Tag::event(EventId::from_hex(&request_id)?),
        ]);

        let event = self.client.sign_event_builder(event_builder).await?;

        self.client.send_event(&event).await?;

        debug!("Sent NWC response to {recipient_pubkey}");
        Ok(())
    }

    /// Send info event with default capabilities
    pub async fn publish_info_event(&self) -> Result<()> {
        let capabilities = [
            "pay_invoice".to_string(),
            "make_invoice".to_string(),
            "get_balance".to_string(),
            "list_transactions".to_string(),
            "get_info".to_string(),
            "pay_keysend".to_string(),
        ];

        let notifications = vec!["payment_received".to_string(), "payment_sent".to_string()];

        let content = capabilities.join(" ");

        let event_builder = EventBuilder::new(Kind::from(13194), content).tags(vec![
            Tag::custom(
                TagKind::Custom("encryption".into()),
                vec!["nip44_v2".to_string(), "nip04".to_string()],
            ),
            Tag::custom(TagKind::Custom("notifications".into()), notifications),
        ]);

        let event = self.client.sign_event_builder(event_builder).await?;

        self.client.send_event(&event).await?;

        info!("Published NWC info event");
        Ok(())
    }

    /// Send notification event
    pub async fn send_notification(
        &self,
        recipient_pubkey: &PublicKey,
        notification_type: &str,
        notification_data: nanduti_core::nwc_protocol::NotificationData,
    ) -> Result<()> {
        use crate::encryption;

        // Create notification payload
        let notification = nanduti_core::nwc_protocol::NostrNotification {
            notification_type: notification_type.to_string(),
            notification: notification_data,
        };

        let content = serde_json::to_string(&notification)?;

        // Encrypt with both NIP-44 and NIP-04 for compatibility
        // Send NIP-44 version (kind 23197)
        let encrypted_nip44 = encryption::encrypt_nip44(&content, recipient_pubkey, &self.keys)?;
        let event_builder_nip44 = EventBuilder::new(Kind::from(23197), encrypted_nip44)
            .tag(Tag::public_key(*recipient_pubkey));
        let event_nip44 = self.client.sign_event_builder(event_builder_nip44).await?;

        self.client.send_event(&event_nip44).await?;

        // Send NIP-04 version for legacy clients (kind 23196)
        let encrypted_nip04 = encryption::encrypt_nip04(&content, recipient_pubkey, &self.keys)?;
        let event_builder_nip04 = EventBuilder::new(Kind::from(23196), encrypted_nip04)
            .tag(Tag::public_key(*recipient_pubkey));
        let event_nip04 = self.client.sign_event_builder(event_builder_nip04).await?;

        self.client.send_event(&event_nip04).await?;

        debug!(
            "Sent {} notification to {}",
            notification_type,
            recipient_pubkey.to_hex()
        );
        Ok(())
    }

    /// Send payment received notification
    pub async fn notify_payment_received(
        &self,
        recipient_pubkey: &PublicKey,
        invoice: &str,
        payment_hash: &str,
        amount_msats: u64,
        preimage: &str,
    ) -> Result<()> {
        use nanduti_core::models::{
            Amount, Bolt11String, PaymentHash, Preimage, Timestamp, TransactionState,
        };
        use nanduti_core::nwc_protocol::{NotificationData, PaymentReceivedNotification};

        let notification_data = NotificationData::PaymentReceived(PaymentReceivedNotification {
            payment_type: "incoming".to_string(),
            state: TransactionState::Settled,
            invoice: Bolt11String::new(invoice.to_string()),
            payment_hash: PaymentHash::new(payment_hash.to_string()),
            preimage: Preimage::new(preimage.to_string()),
            amount: Amount::from_msats(amount_msats),
            settled_at: Timestamp::now(),
        });

        self.send_notification(recipient_pubkey, "payment_received", notification_data)
            .await
    }

    /// Send payment sent notification
    pub async fn notify_payment_sent(
        &self,
        recipient_pubkey: &PublicKey,
        invoice: &str,
        payment_hash: &str,
        amount_msats: u64,
        fees_paid_msats: Option<u64>,
        preimage: &str,
    ) -> Result<()> {
        use nanduti_core::models::{
            Amount, Bolt11String, PaymentHash, Preimage, Timestamp, TransactionState,
        };
        use nanduti_core::nwc_protocol::{NotificationData, PaymentSentNotification};

        let notification_data = NotificationData::PaymentSent(PaymentSentNotification {
            payment_type: "outgoing".to_string(),
            state: TransactionState::Settled,
            invoice: Bolt11String::new(invoice.to_string()),
            payment_hash: PaymentHash::new(payment_hash.to_string()),
            preimage: Preimage::new(preimage.to_string()),
            amount: Amount::from_msats(amount_msats),
            fees_paid: fees_paid_msats.map(Amount::from_msats),
            settled_at: Timestamp::now(),
        });

        self.send_notification(recipient_pubkey, "payment_sent", notification_data)
            .await
    }

    /// Handle incoming NWC events
    pub async fn handle_nwc_events(&self, handler: Arc<crate::NwcHandler>) -> Result<()> {
        use std::collections::HashSet;

        // Track processed events to avoid duplicates
        let mut processed_events = HashSet::new();

        // Subscribe to NWC request events (kind 23194) sent to us
        let filter = Filter::new()
            .kind(Kind::from(23194))
            .pubkey(self.keys.public_key());

        info!(
            "Listening for NWC requests on {} relays",
            self.client.relays().await.len()
        );

        // Use proper event streaming (subscribe and poll)
        self.client.subscribe(filter.clone(), None).await?;

        // Poll for events continuously
        loop {
            // Get new events from database
            let events = self.client.database().query(filter.clone()).await?;

            for event in events {
                // Skip if we've already processed this event
                if processed_events.contains(&event.id) {
                    continue;
                }

                // Mark as processed
                processed_events.insert(event.id);

                // Handle the event
                if let Err(e) = self.handle_single_event(event, handler.clone()).await {
                    tracing::error!("Error handling event: {}", e);
                }

                // Clean up old events if the set gets too large
                if processed_events.len() > 10000 {
                    processed_events.clear();
                }
            }

            // Small delay before next poll
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
    }

    /// Handle a single NWC event
    async fn handle_single_event(
        &self,
        event: Event,
        handler: Arc<crate::NwcHandler>,
    ) -> Result<()> {
        use crate::encryption;
        use nanduti_core::nwc_protocol::NwcRequest;

        debug!("Processing event from {}", event.pubkey);

        // Determine encryption method from tags
        let tags_vec: Vec<Tag> = event.tags.into_iter().collect();
        let encryption_method = encryption::parse_encryption_method(&tags_vec);

        // Decrypt the request
        let decrypted_content = match encryption_method {
            encryption::EncryptionMethod::Nip44 => {
                encryption::decrypt_nip44(&event.content, &event.pubkey, &self.keys)?
            }
            encryption::EncryptionMethod::Nip04 => {
                encryption::decrypt_nip04(&event.content, &event.pubkey, &self.keys)?
            }
        };

        // Parse the request
        let request: NwcRequest =
            serde_json::from_str(&decrypted_content).context("Failed to parse NWC request")?;

        info!("Received NWC request: {}", request.method);

        // Handle the request
        let response = handler.handle_request(request).await?;

        // Serialize response
        let response_content = serde_json::to_string(&response)?;

        // Encrypt the response using the same method
        let encrypted_response = match encryption_method {
            encryption::EncryptionMethod::Nip44 => {
                encryption::encrypt_nip44(&response_content, &event.pubkey, &self.keys)?
            }
            encryption::EncryptionMethod::Nip04 => {
                encryption::encrypt_nip04(&response_content, &event.pubkey, &self.keys)?
            }
        };

        // Send response
        self.send_nwc_response(event.id.to_hex(), event.pubkey.to_hex(), encrypted_response)
            .await?;

        Ok(())
    }
}
