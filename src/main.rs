mod error;
mod health;
mod logger;
mod manager;
mod register;
mod scanner;
mod webserver;

use anyhow::Result;
use async_curl::CurlActor;

use crate::{manager::Manager, webserver::WebServerBuilder};

const BIND_ADDR: &str = "0.0.0.0:5248";

#[tokio::main]
async fn main() -> Result<()> {
    logger::setup_logger();
    log::info!("[auto-server-detection] Started.");

    let web_handler = WebServerBuilder::new().bind_addr(BIND_ADDR).spawn().await?;
    let manager = Manager::new(CurlActor::new()).run().await;

    let _ = tokio::signal::ctrl_c().await;

    web_handler.stop();
    manager.stop();

    let _ = tokio::join!(web_handler.handle, manager.handle);

    log::info!("[auto-server-detection] Ended.");
    Ok(())
}
