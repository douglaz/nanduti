//! MCP server implementation for nanduti-api (placeholder)
//! This module is only compiled when the `mcp` feature is enabled

#[cfg(feature = "mcp")]
pub struct McpServer;

#[cfg(feature = "mcp")]
impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl McpServer {
    pub fn new() -> Self {
        McpServer
    }
}
