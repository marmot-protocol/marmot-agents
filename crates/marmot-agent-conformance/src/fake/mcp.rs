//! An MCP client over streamable HTTP, as an ACP agent is one to the servers a client hands it: just
//! enough to greet a server, list its tools and call one, each a request of its own, as OpenCode,
//! Goose and Cursor do. The ACP fake calls the driver's tool with it, and so do the ACP driver's own
//! tests.

use serde_json::{Value, json};
use std::io;
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

    /// One request: the answer's status and its JSON, `null` when it has none. An error when the
    /// server could not be reached or did not answer in HTTP, as when it stops mid-request.
    pub async fn post(&self, body: &Value) -> io::Result<(u16, Value)> {
        let bad = |what: &str| io::Error::other(what.to_string());
        let rest = (self.url.strip_prefix("http://")).ok_or_else(|| bad("not an http address"))?;
        let (address, path) = rest.split_at(rest.find('/').ok_or_else(|| bad("no path"))?);
        let mut stream = tokio::net::TcpStream::connect(address).await?;
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
        stream.write_all(request.as_bytes()).await?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await?;
        answer(&raw)
    }

    /// One request that must be answered with 200: its JSON.
    async fn ok(&self, body: &Value) -> io::Result<Value> {
        match self.post(body).await? {
            (200, answer) => Ok(answer),
            (status, _) => Err(io::Error::other(format!("answered {status}"))),
        }
    }

    /// The tools the server lists, after greeting it.
    pub async fn tools(&self) -> io::Result<Vec<Value>> {
        let hello = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "fake-agent", "version": "0"}}});
        self.ok(&hello).await?;
        let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        match self.post(&initialized).await? {
            (200 | 202, _) => {}
            (status, _) => return Err(io::Error::other(format!("answered {status}"))),
        }
        let listed = self
            .ok(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
            .await?;
        Ok(listed["result"]["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// Greets the server, finds `tool` among its tools, and calls it with `arguments` and `meta`.
    /// Gives back the call's result, or an error when the server cannot be used or has no such
    /// tool.
    pub async fn call(
        &self,
        tool: &str,
        arguments: Value,
        meta: Option<Value>,
    ) -> io::Result<Value> {
        if !self.tools().await?.iter().any(|t| t["name"] == tool) {
            return Err(io::Error::other("no such tool"));
        }
        let mut params = json!({"name": tool, "arguments": arguments});
        if let Some(meta) = meta {
            params["_meta"] = meta;
        }
        let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": params});
        let answered = self.ok(&call).await?;
        match answered.get("result") {
            Some(result) => Ok(result.clone()),
            None => Err(io::Error::other("the call was not answered with a result")),
        }
    }
}

/// An HTTP answer, as it came: its status and its JSON, `null` when it has none. The body is read
/// as text only once it is whole, since a chunk may end inside a character.
fn answer(raw: &[u8]) -> io::Result<(u16, Value)> {
    let bad = |what: &str| io::Error::other(what.to_string());
    let (head, rest) = split(raw, b"\r\n\r\n").ok_or_else(|| bad("no answer"))?;
    let head = String::from_utf8_lossy(head);
    let status = (head.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad("no status"))?;
    let body = if head.to_lowercase().contains("transfer-encoding: chunked") {
        unchunk(rest)
    } else {
        rest.to_vec()
    };
    Ok((status, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

/// What comes before the first `at`, and what after it.
fn split<'a>(bytes: &'a [u8], at: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    let i = bytes.windows(at.len()).position(|w| w == at)?;
    Some((&bytes[..i], &bytes[i + at.len()..]))
}

/// A chunked body, its framing taken off.
fn unchunk(mut rest: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    while let Some((size, after)) = split(rest, b"\r\n") {
        let size = std::str::from_utf8(size).unwrap_or_default().trim();
        let size = usize::from_str_radix(size, 16).unwrap_or(0);
        if size == 0 || after.len() < size {
            break;
        }
        body.extend_from_slice(&after[..size]);
        rest = after.get(size + 2..).unwrap_or_default();
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    // A chunk counts bytes, and may end inside a character.
    #[test]
    fn a_character_split_between_chunks_is_read_whole() {
        let body = "{\"x\":\"\u{e9}\"}".as_bytes();
        let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for chunk in [&body[..7], &body[7..]] {
            raw.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            raw.extend_from_slice(chunk);
            raw.extend_from_slice(b"\r\n");
        }
        raw.extend_from_slice(b"0\r\n\r\n");
        assert_eq!(answer(&raw).unwrap(), (200, json!({"x": "\u{e9}"})));
    }
}
