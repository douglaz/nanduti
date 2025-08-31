//! MCP server implementation for fedimint-nwc-api (placeholder)
//! This module is only compiled when the `mcp` feature is enabled

#[cfg(feature = "mcp")]
pub struct McpServer;

#[cfg(feature = "mcp")]
impl McpServer {
    pub fn new() -> Self {
        McpServer
    }
}
