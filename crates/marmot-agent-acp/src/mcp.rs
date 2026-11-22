//! The driver's own MCP server, which gives the agent one tool, `send_file` (D-090).
//!
//! ACP lets a client hand an agent MCP servers when it opens a session. OpenCode, Goose and Cursor
//! each take one over streamable HTTP (`mcpCapabilities.http`), so the driver serves one, with the
//! official MCP library (`rmcp`), on the loopback address and a port the system picks. Nothing
//! else on the machine may use it: every request must carry a chat's token, a random secret the
//! driver gives the agent in a header, never in the address, since an agent may log the address.
//!
//! A call goes to the driver as a [`Call`], with what tells which chat made it: the token, which
//! names a chat and the folder its session was opened in, and the session the call itself names in
//! its `_meta`, which OpenCode and Goose add and Cursor does not. The driver decides; this module
//! only carries the call there and the answer back.
//!
//! The driver starts the server unless the settings switch file sending off (D-110).

use marmot_agent_driver::{ChatId, Reason, Refusal};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// The name of the driver's tool, and of the server the agent knows it by.
pub const SEND_FILE: &str = "send_file";

/// The server's name, as the agent shows it: OpenCode calls the tool `marmot_send_file`, Goose
/// `marmot__send_file`, Cursor `marmot-send_file`.
pub const SERVER: &str = "marmot";

/// The keys under which an agent names, in a call's `_meta`, the ACP session that made it: OpenCode
/// 2.3.2 and Goose 1.59.2, read in their sources and seen on the wire 2026-11-22.
const SESSION_KEYS: [&str; 2] = ["ai.opencode/sessionID", "agent-session-id"];

/// How long a connection may take, once the server stops, to finish what it was given. A call
/// whose whole request has arrived is answered in that time; one still waiting for the rest of its
/// request is cut off.
const DRAIN: std::time::Duration = std::time::Duration::from_secs(2);

/// Who a token was given to: a chat, for a session it opened in a folder.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Owner {
    pub chat: ChatId,
    pub folder: String,
}

/// Each token, and who it was given to. The driver adds one for each chat and folder it opens a
/// session in.
pub(crate) type Tokens = Arc<Mutex<HashMap<String, Owner>>>;

/// The session a call names in its `_meta`.
pub(crate) enum Named {
    /// A session, as the agent names it.
    Session(String),
    /// A key for the session, with no session the driver can read in it.
    Malformed,
    /// No key for the session at all.
    Nothing,
}

/// A call of the tool, for the driver to route and answer.
pub(crate) struct Call {
    /// Who the token the call carried was given to. An agent that keeps one server for every
    /// session in a folder (OpenCode) sends the token of whichever chat opened a session there
    /// last.
    pub owner: Owner,
    /// The ACP session the call names in its `_meta`.
    pub session: Named,
    /// The tool's argument, as the agent gave it.
    pub path: Option<String>,
    /// Where the answer goes: sent, or refused and why.
    pub reply: oneshot::Sender<Result<(), Refusal>>,
}

/// The server, running. Dropping it stops it.
pub(crate) struct Server {
    url: String,
    /// The calls the agent makes, in order.
    pub calls: mpsc::UnboundedReceiver<Call>,
    stop: CancellationToken,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl Server {
    /// Starts the server on the loopback address, for the chats `tokens` names.
    pub async fn start(tokens: Tokens) -> io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let url = format!("http://{}/mcp", listener.local_addr()?);
        let (to_driver, calls) = mpsc::unbounded_channel();
        let stop = CancellationToken::new();
        let handler = Handler {
            tokens: tokens.clone(),
            to_driver,
        };
        // Stateless, with plain JSON answers: each request stands alone, and nothing is kept
        // between them but the tokens. rmcp refuses a `Host` that is not the loopback, so a web
        // page cannot reach the server through a name it controls.
        let config = StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_sse_keep_alive(None);
        let service = StreamableHttpService::new(
            move || Ok(handler.clone()),
            Arc::new(NeverSessionManager::default()),
            config,
        );
        tokio::spawn(serve(listener, service, tokens, stop.clone()));
        Ok(Self { url, calls, stop })
    }

    /// What the driver hands the agent on `session/new`, `session/load` and `session/resume`: this
    /// server, with the chat's token.
    pub fn for_session(&self, token: &str) -> serde_json::Value {
        serde_json::json!([{
            "type": "http",
            "name": SERVER,
            "url": self.url,
            "headers": [{"name": "Authorization", "value": format!("Bearer {token}")}],
        }])
    }
}

/// A new token: 128 random bits, as hex.
pub(crate) fn token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Who the token a request carries was given to, if it carries one the driver gave.
fn owner_of(headers: &http::HeaderMap, tokens: &Tokens) -> Option<Owner> {
    let value = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    tokens.lock().ok()?.get(token).cloned()
}

/// Accepts connections until stopped. A request without a token the driver gave is refused before
/// MCP reads it.
async fn serve(
    listener: tokio::net::TcpListener,
    service: StreamableHttpService<Handler, NeverSessionManager>,
    tokens: Tokens,
    stop: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            () = stop.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, _)) = accepted else {
            continue;
        };
        let service = service.clone();
        let tokens = tokens.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let guarded = hyper::service::service_fn(move |request: http::Request<_>| {
                let mut service = service.clone();
                let known = owner_of(request.headers(), &tokens).is_some();
                async move {
                    if !known {
                        let refused = http::Response::builder()
                            .status(http::StatusCode::FORBIDDEN)
                            .body(rmcp_body("Forbidden"))
                            .expect("a fixed response");
                        return Ok::<_, std::convert::Infallible>(refused);
                    }
                    tower_service::Service::call(&mut service, request).await
                }
            });
            let io = hyper_util::rt::TokioIo::new(stream);
            let connection =
                hyper::server::conn::http1::Builder::new().serve_connection(io, guarded);
            let mut connection = std::pin::pin!(connection);
            // Once stopped, a call in flight is still answered, as not sent: the driver that
            // would have answered it has gone. Then the connection closes, by `DRAIN` at the
            // latest, even when its request never arrives whole.
            tokio::select! {
                () = stop.cancelled() => {
                    connection.as_mut().graceful_shutdown();
                    let _ = tokio::time::timeout(DRAIN, connection).await;
                }
                _ = connection.as_mut() => {}
            }
        });
    }
}

/// A body for a response the guard makes itself, of the type rmcp's responses have.
fn rmcp_body(
    text: &'static str,
) -> http_body_util::combinators::BoxBody<bytes::Bytes, std::convert::Infallible> {
    use http_body_util::BodyExt;
    http_body_util::Full::new(bytes::Bytes::from_static(text.as_bytes())).boxed()
}

/// What answers the agent's MCP requests.
#[derive(Clone)]
struct Handler {
    tokens: Tokens,
    to_driver: mpsc::UnboundedSender<Call>,
}

fn tool() -> Tool {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {"path": {"type": "string", "description": "The file to send."}},
        "required": ["path"],
        "additionalProperties": false,
    });
    let serde_json::Value::Object(schema) = schema else {
        unreachable!("a schema is an object")
    };
    Tool::new(
        SEND_FILE,
        "Send a file from the working folder to the chat, as an attachment the people in the chat can open. Give its path, relative to the working folder or absolute.",
        schema,
    )
}

/// The tool's result, in words the model reads.
fn result(sent: &Result<(), Refusal>) -> CallToolResult {
    match sent {
        Ok(()) => CallToolResult::success(vec![ContentBlock::text("Sent.")]),
        Err(refusal) => {
            let text = match &refusal.detail {
                Some(detail) => format!("Not sent: {detail}"),
                None => "Not sent.".to_string(),
            };
            CallToolResult::error(vec![ContentBlock::text(text)])
        }
    }
}

impl rmcp::ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER, env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![tool()]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let parts = context.extensions.get::<http::request::Parts>();
        let Some(owner) = parts.and_then(|p| owner_of(&p.headers, &self.tokens)) else {
            return Err(rmcp::ErrorData::invalid_request("unknown client", None));
        };
        if request.name != SEND_FILE {
            let refused = Refusal {
                reason: Reason::Invalid,
                detail: Some("there is no such tool.".into()),
            };
            return Ok(result(&Err(refused)).into());
        }
        // The session is named in the call's `_meta`, which rmcp hands over as the context's.
        let meta = request.meta.as_ref().map(|m| &m.0.0);
        let meta = meta.or(Some(&context.meta.0.0));
        let named = (SESSION_KEYS.iter()).find_map(|k| meta.and_then(|m| m.get(*k)));
        let session = match named {
            None => Named::Nothing,
            Some(serde_json::Value::String(s)) if !s.is_empty() => Named::Session(s.clone()),
            Some(_) => Named::Malformed,
        };
        let path = (request.arguments.as_ref())
            .and_then(|a| a.get("path"))
            .and_then(|p| p.as_str())
            .map(String::from);
        let (reply, answer) = oneshot::channel();
        let call = Call {
            owner,
            session,
            path,
            reply,
        };
        let sent = match self.to_driver.send(call) {
            Ok(()) => answer
                .await
                .unwrap_or_else(|_| Err(Refusal::new(Reason::Failed))),
            Err(_) => Err(Refusal::new(Reason::Failed)), // the driver has gone
        };
        Ok(result(&sent).into())
    }
}
