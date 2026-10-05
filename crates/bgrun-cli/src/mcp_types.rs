use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON-RPC 2.0 request frame.
#[derive(Deserialize, Debug)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// A JSON-RPC 2.0 response frame.
#[derive(Serialize, Debug)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Option<Value>, code: i64, message: impl Into<String>) -> Self {
        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// A JSON-RPC 2.0 error object.
#[derive(Serialize, Debug)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// An MCP tool definition as returned by `tools/list`.
#[derive(Serialize, Debug)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// The result of an MCP `tools/call`.
#[derive(Serialize, Debug)]
pub struct CallToolResult {
    pub content: Vec<ToolContent>,
    #[serde(rename = "isError", skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

/// Content blocks inside a `CallToolResult`. Text-only for now.
#[derive(Serialize, Debug)]
#[serde(tag = "type")]
pub enum ToolContent {
    #[serde(rename = "text")]
    Text { text: String },
}

impl CallToolResult {
    pub fn success(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text { text: text.into() }],
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text { text: text.into() }],
            is_error: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_ok_shape() {
        let r = JsonRpcResponse::ok(Some(serde_json::json!(1)), serde_json::json!({"ok": true}));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert!(v["result"].is_object());
        assert!(v.get("error").is_none());
    }

    #[test]
    fn tool_result_error_shape() {
        let r = CallToolResult::error("boom");
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["isError"], true);
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][0]["text"], "boom");
    }

    #[test]
    fn tool_result_success_omits_is_error() {
        let r = CallToolResult::success("ok");
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("isError").is_none());
    }
}
