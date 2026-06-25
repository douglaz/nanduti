//! REST API handlers

pub mod federations;
pub mod invoices;
pub mod nwc;
pub mod payments;
pub mod transactions;

#[cfg(test)]
mod test_federations;
#[cfg(test)]
mod test_invoices;
#[cfg(test)]
mod test_nwc;
#[cfg(test)]
mod test_payments;
#[cfg(test)]
mod test_transactions;

pub use federations::*;
pub use invoices::*;
pub use nwc::*;
pub use payments::*;
pub use transactions::*;
