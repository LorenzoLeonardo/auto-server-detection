mod logger;
mod scanner;
mod webserver;

use std::{net::Ipv4Addr, time::Duration};

use anyhow::Result;
use async_curl::CurlActor;
use curl_http_client::{Collector, HttpClient};
use http::{Method, Request, header};
use serde::Serialize;

use crate::{
    scanner::{Error, SubnetScannerBuilder},
    webserver::WebServerBuilder,
};

const BIND_ADDR: &str = "0.0.0.0:5248";

async fn register_device(
    curl: CurlActor<Collector>,
    server_ip: Ipv4Addr,
    port: u16,
    device_ip: Ipv4Addr,
) -> Result<(), Error> {
    let url = format!("https://{server_ip}:{port}/register");
    println!("{url}");

    let collector = Collector::RamAndHeaders(Vec::new(), Vec::new());
    #[derive(Serialize)]
    struct Reg {
        ip: String,
    }

    let body = Reg {
        ip: device_ip.to_string(),
    };

    let request = Request::builder()
        .uri(url.as_str())
        .method(Method::POST)
        .header(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        )
        .body(Some(serde_json::to_vec(&body)?))?;

    let response = HttpClient::new(collector)
        .request(request)?
        .ssl_verify_host(false)?
        .ssl_verify_peer(false)?
        .nonblocking(curl)
        .perform()
        .await?;

    log::info!("{:#?}", response.headers());
    log::info!("{response:#?}");
    Ok(())
}

async fn scan_subnet(curl: CurlActor<Collector>) -> (Ipv4Addr, u16, Ipv4Addr) {
    loop {
        match SubnetScannerBuilder::new()
            .port(5247)
            .timeout(Duration::from_secs(1))
            .scan(curl.clone())
            .await
        {
            Ok((ip, port, device_ip)) => {
                log::info!("Success! Found server at {}:{}", ip, port);
                break (ip, port, device_ip);
            }
            Err(e) => {
                log::error!("Scan failed: {e}. Retrying in 5 second...");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
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
    let (server_ip, port, device_ip) = scan_subnet(curl.clone()).await;
    register_device(curl, server_ip, port, device_ip).await?;

    tokio::signal::ctrl_c().await.unwrap();
    log::info!("Ctrl+C received, shutting down.");
    web_handler.stop();
    web_handler.handle.await.unwrap();
    Ok(())
}
