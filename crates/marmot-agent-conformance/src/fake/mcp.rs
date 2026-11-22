//! An MCP client over streamable HTTP, as an ACP agent is one to the servers a client hands it: just
//! enough to greet a server, list its tools and call one, each a request of its own, as OpenCode,
//! Goose and Cursor do.

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A server as `session/new` hands it over: its address and the headers to send.
#[derive(Clone, Debug)]
pub struct Server {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

impl Server {
    /// The HTTP servers a session request names.
    pub fn from_params(params: &Value) -> Vec<Self> {
        let servers = params["mcpServers"].as_array().into_iter().flatten();
        servers
            .filter(|s| s["type"] == "http")
            .filter_map(|s| {
                let headers = (s["headers"].as_array().into_iter().flatten())
                    .filter_map(|h| Some((h["name"].as_str()?.into(), h["value"].as_str()?.into())))
                    .collect();
                Some(Self {
                    url: s["url"].as_str()?.to_string(),
                    headers,
                })
            })
            .collect()
    }

    /// One request; the answer's JSON, or `None` when there is none or it is not 200.
    async fn post(&self, body: &Value) -> Option<Value> {
        let rest = self.url.strip_prefix("http://")?;
        let (address, path) = rest.split_at(rest.find('/')?);
        let mut stream = tokio::net::TcpStream::connect(address).await.ok()?;
        let body = body.to_string();
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2025-11-25\r\n"
        );
        for (name, value) in &self.headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str(&format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ));
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.ok()?;
        let raw = String::from_utf8_lossy(&raw).into_owned();
        let (head, rest) = raw.split_once("\r\n\r\n")?;
        if head.split(' ').nth(1) != Some("200") {
            return None;
        }
        let text = if head.to_lowercase().contains("transfer-encoding: chunked") {
            unchunk(rest)
        } else {
            rest.to_string()
        };
        serde_json::from_str(&text).ok()
    }

    /// Greets the server, finds `tool` among its tools, and calls it with `arguments` and `meta`.
    /// Gives back the call's result, or `None` when the server cannot be used.
    pub async fn call(&self, tool: &str, arguments: Value, meta: Option<Value>) -> Option<Value> {
        let hello = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "fake-agent", "version": "0"}}});
        self.post(&hello).await?;
        let _ = self
            .post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        let tools = self
            .post(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
            .await?;
        let listed = tools["result"]["tools"].as_array().into_iter().flatten();
        if !listed.into_iter().any(|t| t["name"] == tool) {
            return None;
        }
        let mut params = json!({"name": tool, "arguments": arguments});
        if let Some(meta) = meta {
            params["_meta"] = meta;
        }
        let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": params});
        let answered = self.post(&call).await?;
        answered.get("result").cloned()
    }
}

fn unchunk(mut rest: &str) -> String {
    let mut text = String::new();
    while let Some((size, after)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if size == 0 || after.len() < size {
            break;
        }
        text.push_str(&after[..size]);
        rest = after.get(size + 2..).unwrap_or_default();
    }
    text
}
