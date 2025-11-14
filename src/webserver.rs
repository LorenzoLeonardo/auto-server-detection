use std::process::Command;

use anyhow::Result;
use axum::{Json, Router, body::Body, extract::Request, routing::post};
use serde_json::json;
use tokio::{sync::watch, task::JoinHandle};

use crate::BIND_ADDR;

async fn spawn_http(bind_addr: &str, app: Router, mut stopper: WebServerStopper) {
    log::info!("[webserver] HTTP Webserver started.");
    let listener = tokio::net::TcpListener::bind(bind_addr).await.unwrap();
    log::info!("SSE Push Server running using HTTP at http://{bind_addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = stopper.shutdown_rx.changed().await;
            log::debug!("HTTP: shutdown signal received!");
        })
        .await
        .unwrap();
    log::info!("[webserver] HTTP Webserver ended.");
}

pub struct WebServerHandler {
    pub handle: JoinHandle<()>,
    web_stopper: WebServerStopper,
}

impl WebServerHandler {
    pub fn stop(&self) {
        self.web_stopper.clone().stop();
    }
}

pub struct WebServer {
    bind_addr: String,
    stopper: WebServerStopper,
    app: Router,
}

impl WebServer {
    pub async fn spawn(self) -> WebServerHandler {
        let bind_addr = self.bind_addr.clone();
        let app = self.app.clone();
        let stopper = self.stopper.clone();

        let fut = async move {
            log::info!("No TLS certs provided — starting HTTP server");
            spawn_http(&bind_addr, app, stopper.clone()).await;
        };
        WebServerHandler {
            handle: tokio::spawn(fut),
            web_stopper: self.stopper,
        }
    }
}

pub struct WebServerBuilder {
    bind_addr: Option<String>,
}

impl Default for WebServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl WebServerBuilder {
    pub fn new() -> Self {
        Self { bind_addr: None }
    }

    pub fn bind_addr(mut self, addr: &str) -> Self {
        self.bind_addr = Some(addr.to_string());
        self
    }

    pub async fn spawn(self) -> Result<WebServerHandler> {
        let bind_addr = self.bind_addr.unwrap_or_else(|| BIND_ADDR.to_string());
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let stopper = WebServerStopper {
            shutdown_tx,
            shutdown_rx,
        };
        let app = Router::new().route("/shutdown", post(shutdown_handler));

        let server = WebServer {
            bind_addr,
            stopper,
            app,
        };
        Ok(server.spawn().await)
    }
}

async fn shutdown_handler(_request: Request<Body>) -> Json<serde_json::Value> {
    log::info!("Shutdown request received.");

    // Attempt shutdown depending on OS
    let result = if cfg!(target_os = "windows") {
        // Windows shutdown
        Command::new("shutdown").args(["/s", "/t", "0"]).output()
    } else if cfg!(target_os = "linux") {
        // Linux shutdown (requires root)
        Command::new("shutdown").args(["-h", "now"]).output()
    } else if cfg!(target_os = "macos") {
        // macOS shutdown (requires sudo)
        Command::new("shutdown").args(["-h", "now"]).output()
    } else {
        Err(std::io::Error::other("Unsupported OS"))
    };

    match result {
        Ok(_) => Json(json!({"status": "shutting down"})),
        Err(err) => Json(json!({
            "status": "error",
            "error": err.to_string()
        })),
    }
}

#[derive(Clone)]
pub struct WebServerStopper {
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
}

impl WebServerStopper {
    pub fn stop(self) {
        let _ = self.shutdown_tx.send(true);
    }
}
