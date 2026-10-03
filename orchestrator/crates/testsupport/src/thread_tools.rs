//! The agent's side of the thread tools (`thread-tools/v1`), for the `thread-tools` script of the
//! fake agent: read the grant out of the message, call the thread's MCP endpoint with it as an
//! agent would (rmcp's own client over streamable HTTP), and say what came back as one line.
//!
//! The line is the evidence a test reads from the agent's artifact:
//!
//! ```text
//! thread-tools: tools=get_ui_catalog,turn_output; catalog=<catalogId> v2 <digest>; again unchanged=true
//! thread-tools: tools=get_ui_catalog,turn_output; no catalog: this thread has no UI catalog; answer in text
//! thread-tools: no grant
//! thread-tools: refused: <what the client said>
//! ```
//!
//! [`call_tool`] is what the `tool <name> <json>` script does: list the endpoint's tools and call one
//! (a relayed tool of an attached MCP server, ADR 0024) with a `callId` of the agent's own, and say
//! what came back.
//!
//! [`coordinate`] is what the addressed agent of `mentions/v1` does with `ask_agent` (ADR 0026).
//!
//! [`announce`] is the other thing an agent does with the endpoint: it says "this is my answer"
//! (`turn_output`, ADR 0031).

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ContentBlock, MetaObject, RequestMetaObject};
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

/// Calls `turn_output` with each of `texts` in turn on the endpoint `grant` names, as an agent
/// that announces its answer does (a later call replaces the earlier answer). `Ok` is the
/// results the tool gave, each `delivered` or what it said when it refused; `Err` is a grant that
/// cannot be used, or a call that failed at the protocol level.
///
/// # Errors
///
/// A line that says what went wrong, in the form of [`call_back`]'s (`thread-tools: …`).
pub async fn announce(grant: Option<Value>, texts: &[&str]) -> Result<Vec<String>, String> {
    let Some(grant) = grant else {
        return Err("thread-tools: no grant".to_owned());
    };
    let (Some(url), Some(token)) = (grant["url"].as_str(), grant["token"].as_str()) else {
        return Err("thread-tools: a grant without a url or a token".to_owned());
    };
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_owned()).auth_header(token);
    let client =
        ().serve(StreamableHttpClientTransport::from_config(config))
            .await
            .map_err(|e| format!("thread-tools: refused: {e}"))?;
    let mut results = Vec::new();
    for text in texts {
        let Value::Object(args) = json!({ "text": text }) else {
            unreachable!("arguments are objects")
        };
        let result = client
            .call_tool(CallToolRequestParams::new("turn_output").with_arguments(args))
            .await
            .map_err(|e| format!("thread-tools: turn_output refused: {e}"))?;
        results.push(if result.is_error == Some(true) {
            text_of(&result.content)
        } else if result.structured_content.as_ref().map(|v| &v["delivered"]) == Some(&json!(true))
        {
            "delivered".to_owned()
        } else {
            "not delivered".to_owned()
        });
    }
    Ok(results)
}

/// Calls the tool `name` with `arguments` on the endpoint `grant` names, as an agent that exposes
/// every tool the endpoint lists does: it lists first, and calls only a tool that is listed. The
/// request's `_meta["thread-tools/v1"]` carries `call_id`, the agent's own id for the call.
///
/// The line is the evidence a test reads from the agent's artifact:
///
/// ```text
/// tool websearch__echo: {"text":"x"}
/// tool websearch__fail failed: the tool failed on purpose
/// tool websearch__echo refused: <what the client said>
/// tool websearch__nope not offered; offered=get_ui_catalog,turn_output,websearch__echo
/// tool: no grant
/// ```
pub async fn call_tool(
    grant: Option<Value>,
    name: &str,
    arguments: Value,
    call_id: &str,
) -> String {
    let Some(grant) = grant else {
        return "tool: no grant".to_owned();
    };
    let (Some(url), Some(token)) = (grant["url"].as_str(), grant["token"].as_str()) else {
        return "tool: a grant without a url or a token".to_owned();
    };
    let Value::Object(arguments) = arguments else {
        return format!("tool {name} refused: the arguments are not a JSON object");
    };
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_owned()).auth_header(token);
    let client = match ().serve(StreamableHttpClientTransport::from_config(config)).await {
        Ok(client) => client,
        Err(e) => return format!("tool {name} refused: {e}"),
    };
    let offered: Vec<String> = match client.list_all_tools().await {
        Ok(tools) => tools.iter().map(|t| t.name.to_string()).collect(),
        Err(e) => return format!("tool {name} refused: {e}"),
    };
    if !offered.iter().any(|n| n == name) {
        return format!("tool {name} not offered; offered={}", offered.join(","));
    }
    let Value::Object(meta) = json!({"thread-tools/v1": {"callId": call_id}}) else {
        unreachable!("an object")
    };
    let mut params = CallToolRequestParams::new(name.to_owned()).with_arguments(arguments);
    params.meta = Some(RequestMetaObject(MetaObject(meta)));
    match client.call_tool(params).await {
        Ok(result) if result.is_error == Some(true) => {
            format!("tool {name} failed: {}", text_of(&result.content))
        }
        Ok(result) => format!("tool {name}: {}", text_of(&result.content)),
        Err(e) => format!("tool {name} refused: {e}"),
    }
}

/// What the fake agent's `coordinate <chain> <chain>…` script does with the endpoint: it asks the
/// agents the person mentioned with `ask_agent` (ADR 0026), each in turn and waiting for the
/// answer, as the addressed agent of `mentions/v1` does, and says what came back as one line.
///
/// A `chain` is `[@]agent[!script][>chain]`: `agent` is asked; its message is `coordinate <chain>`
/// when the chain goes on (so the asked agent, a fake too, asks the next one in its turn: an ask
/// of an asked agent), else `<script> work` when a `!script` is given (`coder!slow`: a task that
/// runs until it is cancelled, `coder!gate`: one that waits for the gate), else `echo <agent>`.
/// The call's `callId` is `<call_prefix>:ask-<position>`, stable across a retry of the same
/// task.
///
/// ```text
/// coordinate: coder: completed echo: echo coder | reviewer: failed scripted failure
/// coordinate: coder: refused you can ask only the agents the person mentioned: reviewer
/// coordinate: ask_agent is not offered; offered=get_ui_catalog,turn_output
/// coordinate: no grant
/// ```
pub async fn coordinate(grant: Option<Value>, chains: &[&str], call_prefix: &str) -> String {
    let Some(grant) = grant else {
        return "coordinate: no grant".to_owned();
    };
    let (Some(url), Some(token)) = (grant["url"].as_str(), grant["token"].as_str()) else {
        return "coordinate: a grant without a url or a token".to_owned();
    };
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_owned()).auth_header(token);
    let client = match ().serve(StreamableHttpClientTransport::from_config(config)).await {
        Ok(client) => client,
        Err(e) => return format!("coordinate: refused: {e}"),
    };
    let offered: Vec<String> = match client.list_all_tools().await {
        Ok(tools) => tools.iter().map(|t| t.name.to_string()).collect(),
        Err(e) => return format!("coordinate: refused: {e}"),
    };
    if !offered.iter().any(|n| n == "ask_agent") {
        return format!(
            "coordinate: ask_agent is not offered; offered={}",
            offered.join(",")
        );
    }
    let mut said = Vec::new();
    for (position, chain) in chains.iter().enumerate() {
        let chain = chain.trim_start_matches('@');
        let (this, tail) = chain.split_once('>').unwrap_or((chain, ""));
        let (agent, script) = this.split_once('!').unwrap_or((this, ""));
        let message = if !tail.is_empty() {
            format!("coordinate {tail}")
        } else if !script.is_empty() {
            format!("{script} work")
        } else {
            format!("echo {agent}")
        };
        let Value::Object(arguments) = json!({"agent": agent, "message": message}) else {
            unreachable!("arguments are objects")
        };
        let Value::Object(meta) =
            json!({"thread-tools/v1": {"callId": format!("{call_prefix}:ask-{}", position + 1)}})
        else {
            unreachable!("an object")
        };
        let mut params = CallToolRequestParams::new("ask_agent").with_arguments(arguments);
        params.meta = Some(RequestMetaObject(MetaObject(meta)));
        said.push(match client.call_tool(params).await {
            Err(e) => format!("{agent}: refused {e}"),
            Ok(result) if result.structured_content.is_none() => {
                format!("{agent}: refused {}", text_of(&result.content))
            }
            Ok(result) => {
                let value = result.structured_content.unwrap_or(Value::Null);
                let words = ["text", "question", "error"]
                    .iter()
                    .find_map(|k| value[*k].as_str())
                    .unwrap_or_default();
                format!(
                    "{agent}: {} {words}",
                    value["state"].as_str().unwrap_or("?")
                )
            }
        });
    }
    format!("coordinate: {}", said.join(" | "))
}

/// What one `ask_agent` call gave back.
#[derive(Debug, Clone)]
pub struct AskReply {
    /// The result said `isError`.
    pub is_error: bool,
    /// The structured content: `{ask, agent, state, text?, artifacts?, question?, error?}`; `Null`
    /// for a refusal, which is only text.
    pub value: Value,
    /// The text of the result: the same JSON for an answer, the refusal's words otherwise.
    pub text: String,
}

/// One `ask_agent` call as the agent that holds `grant` makes it (rmcp's own client over
/// streamable HTTP), with `callId` `call_id` in the request's `_meta["thread-tools/v1"]`: it waits
/// for the asked agent to answer. For tests that play the asking agent themselves.
///
/// # Panics
///
/// On a grant that cannot be used and on a call that fails at the protocol level.
pub async fn ask_agent(
    grant: &Value,
    agent: &str,
    message: &str,
    call_id: Option<&str>,
) -> AskReply {
    let (Some(url), Some(token)) = (grant["url"].as_str(), grant["token"].as_str()) else {
        panic!("a grant without a url or a token: {grant}");
    };
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_owned()).auth_header(token);
    let client =
        ().serve(StreamableHttpClientTransport::from_config(config))
            .await
            .expect("the MCP handshake");
    let Value::Object(arguments) = json!({"agent": agent, "message": message}) else {
        unreachable!("arguments are objects")
    };
    let mut params = CallToolRequestParams::new("ask_agent").with_arguments(arguments);
    if let Some(id) = call_id {
        let Value::Object(meta) = json!({"thread-tools/v1": {"callId": id}}) else {
            unreachable!("an object")
        };
        params.meta = Some(RequestMetaObject(MetaObject(meta)));
    }
    let result = client
        .call_tool(params)
        .await
        .unwrap_or_else(|e| panic!("ask_agent failed at the protocol level: {e}"));
    AskReply {
        is_error: result.is_error == Some(true),
        text: text_of(&result.content),
        value: result.structured_content.unwrap_or(Value::Null),
    }
}
