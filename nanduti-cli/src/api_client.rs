//! REST API client for the nanduti server

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// REST API client for communicating with the nanduti server
pub struct ApiClient {
    client: Client,
    base_url: String,
}

impl ApiClient {
    /// Create a new API client
    pub fn new(base_url: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self { client, base_url })
    }

    /// Check server health
    #[allow(dead_code)]
    pub async fn health(&self) -> Result<()> {
        let response = self
            .client
            .get(format!("{}/health", self.base_url))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("Server health check failed"));
        }

        Ok(())
    }

    // Federation management

    /// Add a new federation
    pub async fn add_federation(&self, invite_code: String) -> Result<AddFederationResponse> {
        let request = AddFederationRequest { invite_code };

        let response = self
            .client
            .post(format!("{}/api/v1/federations", self.base_url))
            .json(&request)
            .send()
            .await?;

        handle_response(response).await
    }

    /// List all federations
    pub async fn list_federations(&self) -> Result<Vec<FederationInfo>> {
        let response = self
            .client
            .get(format!("{}/api/v1/federations", self.base_url))
            .send()
            .await?;

        handle_response(response).await
    }

    /// Get federation details
    pub async fn get_federation(&self, id: &str) -> Result<FederationInfo> {
        let response = self
            .client
            .get(format!("{}/api/v1/federations/{}", self.base_url, id))
            .send()
            .await?;

        handle_response(response).await
    }

    /// Remove a federation
    pub async fn remove_federation(&self, id: &str) -> Result<()> {
        let response = self
            .client
            .delete(format!("{}/api/v1/federations/{}", self.base_url, id))
            .send()
            .await?;

        if !response.status().is_success() {
            let error = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow!("Failed to remove federation: {}", error));
        }

        Ok(())
    }

    /// Get federation balance
    #[allow(dead_code)]
    pub async fn get_federation_balance(&self, id: &str) -> Result<Value> {
        let response = self
            .client
            .get(format!(
                "{}/api/v1/federations/{}/balance",
                self.base_url, id
            ))
            .send()
            .await?;

        handle_response(response).await
    }

    /// List federation gateways
    pub async fn list_federation_gateways(&self, id: &str) -> Result<Vec<GatewayInfo>> {
        let response = self
            .client
            .get(format!(
                "{}/api/v1/federations/{}/gateways",
                self.base_url, id
            ))
            .send()
            .await?;

        handle_response(response).await
    }

    // Invoice management

    /// Create a new invoice
    pub async fn create_invoice(
        &self,
        request: CreateInvoiceRequest,
    ) -> Result<CreateInvoiceResponse> {
        let response = self
            .client
            .post(format!("{}/api/v1/invoices", self.base_url))
            .json(&request)
            .send()
            .await?;

        handle_response(response).await
    }

    // Payment management

    /// Pay an invoice
    pub async fn pay_invoice(&self, request: PayInvoiceRequest) -> Result<PayInvoiceResponse> {
        let response = self
            .client
            .post(format!("{}/api/v1/payments", self.base_url))
            .json(&request)
            .send()
            .await?;

        handle_response(response).await
    }

    // Transaction management

    /// List transactions
    pub async fn list_transactions(
        &self,
        federation_id: Option<String>,
        limit: Option<usize>,
    ) -> Result<Vec<TransactionInfo>> {
        let mut url = format!("{}/api/v1/transactions", self.base_url);
        let mut params = Vec::new();

        if let Some(id) = federation_id {
            params.push(format!("federation_id={}", id));
        }
        if let Some(limit) = limit {
            params.push(format!("limit={}", limit));
        }

        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = self.client.get(url).send().await?;

        handle_response(response).await
    }

    // NWC connection management

    /// Create a new NWC connection
    pub async fn create_nwc_connection(
        &self,
        request: CreateConnectionRequest,
    ) -> Result<CreateConnectionResponse> {
        let response = self
            .client
            .post(format!("{}/api/v1/nwc/connections", self.base_url))
            .json(&request)
            .send()
            .await?;

        handle_response(response).await
    }

    /// List NWC connections
    pub async fn list_nwc_connections(&self) -> Result<Vec<ConnectionInfo>> {
        let response = self
            .client
            .get(format!("{}/api/v1/nwc/connections", self.base_url))
            .send()
            .await?;

        handle_response(response).await
    }
}

/// Handle API response and error extraction
async fn handle_response<T: for<'de> Deserialize<'de>>(response: reqwest::Response) -> Result<T> {
    let status = response.status();

    if status.is_success() {
        let data = response.json::<T>().await?;
        Ok(data)
    } else {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        Err(anyhow!("API error ({}): {}", status, error_text))
    }
}

// Request/Response types matching the server handlers

#[derive(Debug, Serialize)]
pub struct AddFederationRequest {
    pub invite_code: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AddFederationResponse {
    pub federation_id: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FederationInfo {
    pub id: String,
    pub name: String,
    pub balance: serde_json::Value, // Will deserialize as Amount in JSON
    pub status: String,             // Will deserialize as FederationStatus string
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GatewayInfo {
    pub gateway_id: String,
    pub api: String,
    pub base_fee_msat: u32,
    pub proportional_fee_ppm: u32,
}

#[derive(Debug, Serialize)]
pub struct CreateInvoiceRequest {
    pub federation_id: Option<String>,
    pub amount: String,
    pub description: String,
    pub expiry: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateInvoiceResponse {
    pub invoice: String,
    pub payment_hash: String,
    pub amount_sats: u64,
    pub amount_msats: u64,
    pub federation_id: String,
}

#[derive(Debug, Serialize)]
pub struct PayInvoiceRequest {
    pub federation_id: Option<String>,
    pub invoice: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PayInvoiceResponse {
    pub payment_hash: String,
    pub preimage: String,
    pub amount_paid_msats: u64,
    pub fees_paid_msats: Option<u64>,
    pub federation_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TransactionInfo {
    pub id: String,
    pub federation_id: String,
    pub transaction_type: String,
    pub state: String,
    pub amount_sats: u64,
    pub description: Option<String>,
    pub payment_hash: String,
    pub created_at: u64,
    pub settled_at: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct CreateConnectionRequest {
    pub name: String,
    pub daily_limit_sats: Option<u64>,
    pub per_payment_limit_sats: Option<u64>,
    pub allowed_federations: Vec<String>,
    pub relays: Vec<String>,
    pub lud16: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateConnectionResponse {
    pub connection_id: String,
    pub name: String,
    pub pubkey: String,
    pub connection_uri: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub name: String,
    pub pubkey: String,
    pub created_at: u64,
    pub last_used: Option<u64>,
    pub total_spent_msats: u64,
}
