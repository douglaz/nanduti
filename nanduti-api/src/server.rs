//! API server implementation

use anyhow::Result;
use axum::Router;
use std::net::SocketAddr;
use tracing::info;

/// Main API server
pub struct Server {
    router: Router,
    addr: SocketAddr,
}

impl Server {
    /// Create a new server instance
    pub fn new(router: Router, addr: SocketAddr) -> Self {
        Self { router, addr }
    }

    /// Start the server
    pub async fn run(self) -> Result<()> {
        info!("Starting API server on {}", self.addr);

        let listener = tokio::net::TcpListener::bind(self.addr).await?;
        axum::serve(listener, self.router).await?;

        Ok(())
    }
}
