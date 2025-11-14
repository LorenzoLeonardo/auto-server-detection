mod logger;
mod manager;
mod register;
mod scanner;
mod webserver;

use std::{net::Ipv4Addr, time::Duration};

use anyhow::Result;
use async_curl::CurlActor;
use curl_http_client::{Collector, HttpClient};
use http::{Method, Request, StatusCode};

use crate::{manager::Manager, scanner::Error, webserver::WebServerBuilder};

const BIND_ADDR: &str = "0.0.0.0:5248";

async fn health_check(
    curl: &CurlActor<Collector>,
    server_ip: Ipv4Addr,
    port: u16,
) -> Result<(), Error> {
    let url = format!("https://{server_ip}:{port}");
    let collector = Collector::RamAndHeaders(Vec::new(), Vec::new());
    let req = Request::builder()
        .uri(url)
        .method(Method::GET)
        .body(None::<Vec<u8>>)
        .unwrap();

    let timeout_duration = Duration::from_secs(3);

    let resp = tokio::time::timeout(timeout_duration, async {
        HttpClient::new(collector)
            .request(req)?
            .ssl_verify_host(false)?
            .ssl_verify_peer(false)?
            .nonblocking(curl.clone())
            .perform()
            .await
    })
    .await;

    match resp {
        Ok(Ok(response)) if response.status() == StatusCode::OK => Ok(()),
        Ok(Ok(response)) => Err(Error::Other(format!(
            "Health check failed with status: {}",
            response.status()
        ))),
        Ok(Err(e)) => Err(Error::Curl(e)),
        Err(_) => Err(Error::Other("Health check timed out".into())),
    }
}

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
