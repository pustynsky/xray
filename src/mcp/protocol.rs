use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── JSON-RPC 2.0 base types ────────────────────────────────────────

/// Incoming JSON-RPC request (may be a notification if id is None)
#[derive(Deserialize, Debug)]
pub struct JsonRpcRequest {
    /// JSON-RPC protocol version. Must equal `"2.0"` per JSON-RPC 2.0 §4;
    /// validated by the dispatcher in [`crate::mcp::server`].
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// Outgoing JSON-RPC response
#[derive(Serialize, Debug)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    pub result: Value,
}

/// Outgoing JSON-RPC error response
#[derive(Serialize, Debug)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: String,
    pub id: Value,
    pub error: JsonRpcError,
}

#[derive(Serialize, Debug)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

// ─── MCP Initialize types ───────────────────────────────────────────

#[derive(Serialize, Debug)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
    /// MCP server-level instructions for LLM clients.
    /// Provides best practices and tool selection guidance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct ServerCapabilities {
    pub tools: ToolsCapability,
}

#[derive(Serialize, Debug)]
pub struct ToolsCapability {
    #[serde(rename = "listChanged")]
    pub list_changed: bool,
}

#[derive(Serialize, Debug)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

// ─── MCP Tools types ────────────────────────────────────────────────

#[derive(Serialize, Debug)]
pub struct ToolsListResult {
    pub tools: Vec<ToolDefinition>,
}

#[derive(Serialize, Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// MCP tool call result content
#[must_use]
#[derive(Serialize, Debug)]
pub struct ToolCallResult {
    pub content: Vec<ToolContent>,
    #[serde(rename = "isError", skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

#[derive(Serialize, Debug)]
pub struct ToolContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

// ─── Helper constructors ────────────────────────────────────────────

impl JsonRpcResponse {
    pub fn new(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result,
        }
    }
}

impl JsonRpcErrorResponse {
    pub fn new(id: Value, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            error: JsonRpcError { code, message },
        }
    }
}

impl ToolCallResult {
    pub fn success(text: String) -> Self {
        Self {
            content: vec![ToolContent {
                content_type: "text".to_string(),
                text,
            }],
            is_error: false,
        }
    }

    pub fn error(text: String) -> Self {
        Self {
            content: vec![ToolContent {
                content_type: "text".to_string(),
                text,
            }],
            is_error: true,
        }
    }
}

impl InitializeResult {
    /// Create a new InitializeResult with dynamic instructions.
    ///
    /// `def_extensions` — the file extensions that have definition parser support
    /// in the current server configuration (intersection of --ext and definition_extensions()).
    #[cfg(test)]
    pub fn new(def_extensions: &[&str]) -> Self {
        Self::new_with_extensions(def_extensions, def_extensions)
    }

    /// Create a new InitializeResult with content-index and definition-parser
    /// extension sets kept separate for language-aware instructions.
    #[cfg(test)]
    pub fn new_with_extensions(content_extensions: &[&str], def_extensions: &[&str]) -> Self {
        let profile = crate::tips::LanguageProfile::new(content_extensions, def_extensions);
        Self::new_with_profile(&profile)
    }

    /// Create a new InitializeResult with explicit XML on-demand availability.
    /// XML on-demand can answer exact file requests even when XML extensions are
    /// not part of the content index, so server startup passes the runtime flag.
    pub fn new_with_extensions_and_xml_on_demand(
        content_extensions: &[&str],
        def_extensions: &[&str],
        xml_on_demand_available: bool,
    ) -> Self {
        let profile = crate::tips::LanguageProfile::new_with_xml_on_demand(
            content_extensions,
            def_extensions,
            xml_on_demand_available,
        );
        Self::new_with_profile(&profile)
    }

    fn new_with_profile(profile: &crate::tips::LanguageProfile) -> Self {
        Self {
            protocol_version: "2025-03-26".to_string(),
            capabilities: ServerCapabilities {
                tools: ToolsCapability {
                    list_changed: false,
                },
            },
            server_info: ServerInfo {
                name: "xray".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            instructions: Some(crate::tips::render_instructions_for_profile(profile)),
        }
    }

    // Instructions text is generated by crate::tips::render_instructions().
    // Note: CLI `xray tips` uses render_cli() and MCP `xray_help` uses render_json() —
    // each renderer is tailored to its audience (machine vs human vs on-demand reference).
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
