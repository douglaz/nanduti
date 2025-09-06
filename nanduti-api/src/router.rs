//! Federation routing logic for optimal payment paths

use anyhow::{anyhow, bail, Result};
use nanduti_core::{
    federation::{Federation, FederationManager, FederationStatus},
    models::Amount,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::{debug, info};

/// Strategy for selecting federations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoutingStrategy {
    LowestFee,
    BestRoute,
    RoundRobin,
    BalanceWeighted,
}

impl std::str::FromStr for RoutingStrategy {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "lowest-fee" => Ok(Self::LowestFee),
            "best-route" => Ok(Self::BestRoute),
            "round-robin" => Ok(Self::RoundRobin),
            "balance-weighted" => Ok(Self::BalanceWeighted),
            _ => bail!("Invalid routing strategy: {s}"),
        }
    }
}

/// Routes payments to optimal federations
pub struct FederationRouter {
    federation_manager: Arc<FederationManager>,
    strategy: RoutingStrategy,
    round_robin_counter: AtomicUsize,
}

impl FederationRouter {
    /// Create a new router
    pub fn new(federation_manager: Arc<FederationManager>, strategy: RoutingStrategy) -> Self {
        Self {
            federation_manager,
            strategy,
            round_robin_counter: AtomicUsize::new(0),
        }
    }

    /// Select a federation for payment
    pub async fn select_federation(&self, amount: Amount) -> Result<Federation> {
        let federations = self.federation_manager.list_federations().await;

        // Filter to online federations with sufficient balance
        let available: Vec<Federation> = federations
            .into_iter()
            .filter(|f| f.status == FederationStatus::Online)
            .filter(|f| f.balance >= amount)
            .collect();

        if available.is_empty() {
            bail!("No federation available to pay {} sats", amount.as_sats());
        }

        match self.strategy {
            RoutingStrategy::LowestFee => self.select_lowest_fee(available, amount).await,
            RoutingStrategy::BestRoute => self.select_best_route(available, amount).await,
            RoutingStrategy::RoundRobin => self.select_round_robin(available),
            RoutingStrategy::BalanceWeighted => self.select_balance_weighted(available).await,
        }
    }

    /// Select a federation for receiving payments
    pub async fn select_federation_for_receive(&self) -> Result<Federation> {
        let federations = self.federation_manager.list_federations().await;

        // Filter to online federations
        let available: Vec<Federation> = federations
            .into_iter()
            .filter(|f| f.status == FederationStatus::Online)
            .collect();

        if available.is_empty() {
            bail!("No federation available to receive payments");
        }

        // For receiving, prefer least loaded federation (most balance available)
        available
            .into_iter()
            .max_by_key(|f| f.balance.as_msats())
            .ok_or_else(|| anyhow!("No federation selected"))
    }

    /// Select federation with lowest fees
    async fn select_lowest_fee(
        &self,
        federations: Vec<Federation>,
        amount: Amount,
    ) -> Result<Federation> {
        let mut best_federation = None;
        let mut lowest_fee = Amount::from_msats(u64::MAX);

        for federation in federations {
            if let Some(client) = &federation.client {
                match client.estimate_fee(amount).await {
                    Ok(fee) if fee < lowest_fee => {
                        lowest_fee = fee;
                        best_federation = Some(federation);
                    }
                    Err(e) => {
                        debug!(
                            "Failed to estimate fee for federation {}: {e}",
                            federation.id
                        );
                    }
                    _ => {}
                }
            }
        }

        best_federation.ok_or_else(|| anyhow!("No federation with fee estimate available"))
    }

    /// Select federation with best route probability
    async fn select_best_route(
        &self,
        federations: Vec<Federation>,
        _amount: Amount,
    ) -> Result<Federation> {
        // Select based on success rate metrics
        federations
            .into_iter()
            .max_by(|a, b| {
                let a_score = a.metrics.success_rate * a.metrics.uptime_percent;
                let b_score = b.metrics.success_rate * b.metrics.uptime_percent;
                a_score
                    .partial_cmp(&b_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .ok_or_else(|| anyhow!("No federation available"))
    }

    /// Select federation using round-robin
    fn select_round_robin(&self, federations: Vec<Federation>) -> Result<Federation> {
        if federations.is_empty() {
            bail!("No federations available");
        }

        let index = self.round_robin_counter.fetch_add(1, Ordering::Relaxed);
        let selected = federations[index % federations.len()].clone();

        info!("Round-robin selected federation: {}", selected.id);
        Ok(selected)
    }

    /// Select federation weighted by balance
    async fn select_balance_weighted(&self, federations: Vec<Federation>) -> Result<Federation> {
        // Calculate total balance
        let total_balance: u64 = federations.iter().map(|f| f.balance.as_msats()).sum();

        if total_balance == 0 {
            bail!("No balance available in any federation");
        }

        // Use weighted random selection
        let random_point = (rand::random::<f64>() * total_balance as f64) as u64;
        let mut accumulated = 0u64;

        for federation in federations {
            accumulated += federation.balance.as_msats();
            if accumulated >= random_point {
                let federation_id = &federation.id;
                let balance = federation.balance.as_msats();
                info!("Balance-weighted selected federation: {federation_id} (balance: {balance} msats)");
                return Ok(federation);
            }
        }

        bail!("Failed to select federation using balance weighting");
    }

    /// Attempt payment with fallback federations
    pub async fn pay_with_fallback(
        &self,
        amount: Amount,
        payment_fn: impl Fn(Federation) -> Result<()>,
    ) -> Result<()> {
        let mut federations = self
            .federation_manager
            .get_payable_federations(amount)
            .await;

        if federations.is_empty() {
            bail!("No federation can pay {} msats", amount.as_msats());
        }

        // Sort by preference based on strategy
        match self.strategy {
            RoutingStrategy::LowestFee => {
                // Estimate actual fees for each federation
                let mut federations_with_fees = Vec::new();
                for federation in federations {
                    if let Some(client) = &federation.client {
                        match client.estimate_fee(amount).await {
                            Ok(fee) => {
                                federations_with_fees.push((federation, fee));
                            }
                            Err(e) => {
                                debug!(
                                    "Failed to estimate fee for federation {}: {e}",
                                    federation.id
                                );
                                // Include with max fee as fallback
                                federations_with_fees
                                    .push((federation, Amount::from_msats(u64::MAX)));
                            }
                        }
                    } else {
                        // No client available, put at the end
                        federations_with_fees.push((federation, Amount::from_msats(u64::MAX)));
                    }
                }

                // Sort by estimated fees
                federations_with_fees.sort_by_key(|(_, fee)| fee.as_msats());

                // Extract sorted federations
                federations = federations_with_fees.into_iter().map(|(f, _)| f).collect();
            }
            RoutingStrategy::BestRoute => {
                federations.sort_by(|a, b| {
                    let a_score = a.metrics.success_rate * a.metrics.uptime_percent;
                    let b_score = b.metrics.success_rate * b.metrics.uptime_percent;
                    b_score
                        .partial_cmp(&a_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            _ => {} // Keep original order
        }

        // Try each federation until one succeeds
        let mut last_error = None;
        for federation in federations {
            info!("Attempting payment via federation: {}", federation.id);
            match payment_fn(federation.clone()) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    debug!("Payment failed via federation {}: {e}", federation.id);
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("All payment attempts failed")))
    }
}

// Add rand for balance-weighted selection
use rand;
