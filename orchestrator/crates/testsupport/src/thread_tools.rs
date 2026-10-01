//! The agent's side of the thread tools (`thread-tools/v1`), for the `thread-tools` script of the
//! fake agent: read the grant out of the message, call the thread's MCP endpoint with it as an
//! agent would (rmcp's own client over streamable HTTP), and say what came back as one line.
//!
//! The line is the evidence a test reads from the agent's artifact:
//!
//! ```text
//! thread-tools: tools=get_ui_catalog; catalog=<catalogId> v2 <digest>; again unchanged=true
//! thread-tools: tools=get_ui_catalog; no catalog: this thread has no UI catalog; answer in text
//! thread-tools: no grant
//! thread-tools: refused: <what the client said>
//! ```

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use serde_json::{Value, json};

/// The text of the first content block of a result: what a tool error says.
fn text_of(content: &[ContentBlock]) -> String {
    content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Calls the endpoint `grant` names: lists the tools, then `get_ui_catalog` without arguments, and
/// again with the digest it was given (the answer to that is `unchanged: true`).
pub async fn call_back(grant: Option<Value>) -> String {
    let Some(grant) = grant else {
        return "thread-tools: no grant".to_owned();
    };
    let (Some(url), Some(token)) = (grant["url"].as_str(), grant["token"].as_str()) else {
        return "thread-tools: a grant without a url or a token".to_owned();
    };
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_owned()).auth_header(token);
    let client = match ().serve(StreamableHttpClientTransport::from_config(config)).await {
        Ok(client) => client,
        Err(e) => return format!("thread-tools: refused: {e}"),
    };
    let tools = match client.list_all_tools().await {
        Ok(tools) => tools
            .iter()
            .map(|t| t.name.to_string())
            .collect::<Vec<_>>()
            .join(","),
        Err(e) => return format!("thread-tools: refused: {e}"),
    };
    let call = |args: Value| {
        let client = &client;
        async move {
            let Value::Object(args) = args else {
                unreachable!("arguments are objects")
            };
            client
                .call_tool(CallToolRequestParams::new("get_ui_catalog").with_arguments(args))
                .await
        }
    };
    let first = match call(json!({})).await {
        Ok(result) => result,
        Err(e) => return format!("thread-tools: tools={tools}; refused: {e}"),
    };
    if first.is_error == Some(true) {
        return format!(
            "thread-tools: tools={tools}; no catalog: {}",
            text_of(&first.content)
        );
    }
    let got = first.structured_content.unwrap_or(Value::Null);
    let digest = got["digest"].as_str().unwrap_or_default().to_owned();
    let mut line = format!(
        "thread-tools: tools={tools}; catalog={} v{} {digest}",
        got["catalogId"].as_str().unwrap_or_default(),
        got["version"],
    );
    match call(json!({ "knownDigest": digest })).await {
        Ok(again) => {
            let unchanged = again.structured_content.unwrap_or(Value::Null)["unchanged"].clone();
            line.push_str(&format!("; again unchanged={unchanged}"));
        }
        Err(e) => line.push_str(&format!("; again refused: {e}")),
    }
    line
}
