//! Core library for multi-federation Fedimint wallet with NWC support

pub mod federation;
pub mod fedimint_client;
pub mod keys;
pub mod lightning;
pub mod mnemonic_store;
pub mod models;
pub mod nwc_protocol;
pub mod storage;

// Re-export main types
pub use federation::{Federation, FederationManager, FederationStatus};
pub use lightning::{LightningOperation, PaymentResult};
pub use models::{Amount, Invoice, Transaction};
pub use nwc_protocol::{NwcMethod, NwcRequest, NwcResponse};
pub use storage::Storage;
