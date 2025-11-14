mod error;
mod health;
mod logger;
mod manager;
mod register;
mod scanner;
mod webserver;

use anyhow::Result;
use async_curl::CurlActor;
use curl_http_client::Collector;

use crate::{error::Error, manager::Manager, webserver::WebServerBuilder};

const BIND_ADDR: &str = "0.0.0.0:5248";

#[tokio::main]
async fn main() -> Result<(), Error> {
    logger::setup_logger();

    let web_handler = WebServerBuilder::new()
        .bind_addr(BIND_ADDR)
        .spawn()
        .await
        .unwrap();

    let curl: CurlActor<Collector> = CurlActor::new();

    let manager = Manager::new(curl);
    tokio::select! {
        _ = manager.run() => {}
        _ = tokio::signal::ctrl_c() => {
            log::info!("Ctrl+C received, shutting down...");
        }
    }

    web_handler.stop();
    let _ = web_handler.handle.await;

    log::info!("Shutdown complete.");
    Ok(())
}
