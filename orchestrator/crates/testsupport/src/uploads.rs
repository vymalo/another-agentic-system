//! The agent's side of `uploads/v1` (ADR 0058, `docs/api/uploads-v1.md`), for the `upload` script of
//! the fake agent: ask the thread's endpoint for a slot with `request_upload`, send the bytes to the
//! URL it gave with the method and the headers it listed, as any HTTP client does, and hand back the
//! id to name in the artifact.
//!
//! The line is the evidence a test reads from the agent's artifact. It never holds the URL or the id,
//! which are secrets for their minutes:
//!
//! ```text
//! upload: slot for report.txt (6144 bytes); put 200
//! upload: slot for report.txt (6144 bytes); put 400
//! upload: slot for report.txt (6144 bytes); nothing put
//! upload: refused: too_large: a file of 10737418240 bytes is more than ...
//! upload: request_upload is not offered; offered=get_ui_catalog,turn_output
//! upload: no grant
//! ```

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

/// What the agent puts at the URL it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Put {
    /// The bytes it declared.
    Declared,
    /// Other bytes of the declared size: an agent that lies about its file.
    Other,
    /// Nothing: an agent that names an upload it never made.
    Nothing,
}

/// What an attempt did: the line for the artifact and the id to name, when a slot was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadAttempt {
    /// One line, with no URL and no id.
    pub line: String,
    /// The id of the slot, `None` when none was given.
    pub upload_id: Option<String>,
}

impl UploadAttempt {
    fn said(line: impl Into<String>) -> Self {
        UploadAttempt {
            line: line.into(),
            upload_id: None,
        }
    }
}

fn text_of(content: &[ContentBlock]) -> String {
    content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Asks the endpoint `grant` names for a slot for `bytes` (named `filename`, of `media_type`), and
/// sends them, as `put` says. `declared_size` replaces the size told to the tool (a file the agent
/// says is bigger than the message allowed).
pub async fn upload_file(
    grant: Option<Value>,
    filename: &str,
    media_type: &str,
    bytes: &[u8],
    put: Put,
    declared_size: Option<u64>,
) -> UploadAttempt {
    let Some(grant) = grant else {
        return UploadAttempt::said("upload: no grant");
    };
    let (Some(url), Some(token)) = (grant["url"].as_str(), grant["token"].as_str()) else {
        return UploadAttempt::said("upload: a grant without a url or a token");
    };
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_owned()).auth_header(token);
    let client = match ().serve(StreamableHttpClientTransport::from_config(config)).await {
        Ok(client) => client,
        Err(e) => return UploadAttempt::said(format!("upload: refused: {e}")),
    };
    let offered: Vec<String> = match client.list_all_tools().await {
        Ok(tools) => tools.iter().map(|t| t.name.to_string()).collect(),
        Err(e) => return UploadAttempt::said(format!("upload: refused: {e}")),
    };
    if !offered.iter().any(|n| n == "request_upload") {
        return UploadAttempt::said(format!(
            "upload: request_upload is not offered; offered={}",
            offered.join(",")
        ));
    }

    let size = declared_size.unwrap_or(bytes.len() as u64);
    let hash: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let Value::Object(arguments) = json!({
        "filename": filename,
        "mediaType": media_type,
        "size": size,
        "sha256": hash,
    }) else {
        unreachable!("an object")
    };
    let result = match client
        .call_tool(CallToolRequestParams::new("request_upload").with_arguments(arguments))
        .await
    {
        Ok(result) => result,
        Err(e) => return UploadAttempt::said(format!("upload: refused: {e}")),
    };
    if result.is_error == Some(true) {
        return UploadAttempt::said(format!("upload: refused: {}", text_of(&result.content)));
    }
    let Some(slot) = result.structured_content else {
        return UploadAttempt::said("upload: the slot has no structured content");
    };
    let (Some(upload_id), Some(put_url)) = (slot["uploadId"].as_str(), slot["url"].as_str()) else {
        return UploadAttempt::said("upload: the slot has no uploadId or url");
    };
    let upload_id = upload_id.to_owned();
    let slot_line = format!("upload: slot for {filename} ({size} bytes)");

    let body = match put {
        Put::Nothing => {
            return UploadAttempt {
                line: format!("{slot_line}; nothing put"),
                upload_id: Some(upload_id),
            };
        }
        Put::Declared => bytes.to_vec(),
        Put::Other => bytes.iter().map(|b| b.wrapping_add(1)).collect(),
    };
    let Ok(http) = reqwest::Client::builder().no_proxy().build() else {
        return UploadAttempt::said("upload: no HTTP client");
    };
    let mut request = http.put(put_url);
    // exactly the headers the slot lists, with exactly those values
    if let Some(headers) = slot["headers"].as_object() {
        for (name, value) in headers {
            if let Some(value) = value.as_str() {
                request = request.header(name, value);
            }
        }
    }
    let line = match request.body(body).send().await {
        Ok(response) => format!("{slot_line}; put {}", response.status().as_u16()),
        Err(e) => format!("{slot_line}; put failed: {}", e.without_url()),
    };
    UploadAttempt {
        line,
        upload_id: Some(upload_id),
    }
}
