mod logger;
mod scanner;
mod webserver;

use std::{net::Ipv4Addr, time::Duration};

use anyhow::Result;
use async_curl::CurlActor;
use curl_http_client::{Collector, HttpClient};
use http::{Method, Request, StatusCode, header};
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
    if response.status() == StatusCode::OK {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "Device registration failed with status: {}",
            response.status()
        )))
    }
}

async fn scan_subnet(curl: CurlActor<Collector>) -> (Ipv4Addr, u16, Ipv4Addr) {
    loop {
        match SubnetScannerBuilder::new()
            .port(5247)
            .timeout(Duration::from_secs(1))
            .scan(curl.clone())
            .await
        {
            Ok(found) => {
                log::info!("Success! Found server at {}:{}", found.0, found.1);
                return found;
            }
            Err(e) => {
                log::error!("Scan failed: {e}. Retrying in 5 seconds...");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

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

async fn health_monitor_loop(
    curl: CurlActor<Collector>,
    server_ip: Ipv4Addr,
    port: u16,
    mut shutdown_signal: tokio::sync::watch::Receiver<()>,
) -> bool {
    loop {
        tokio::select! {
            _ = shutdown_signal.changed() => {
                log::info!("Shutdown signal received, stopping health monitor.");
                return true;
            }

            res = async {
                match health_check(&curl, server_ip, port).await {
                    Ok(_) => {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        Ok(())
                    }
                    Err(e) => {
                        log::error!("Health check failed: {e}");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        Err(e)
                    }
                }
            } => {
                if let Err(e) = res {
                    log::warn!("Health check error, exiting loop to rescan: {e}");
                    return false;
                }
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

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(());

    let main_task = tokio::spawn(async move {
        loop {
            let (server_ip, port, device_ip) = scan_subnet(curl.clone()).await;

            match register_device(curl.clone(), server_ip, port, device_ip).await {
                Ok(_) => log::info!("Device registered successfully."),
                Err(e) => {
                    log::error!("Registration failed: {e}");
                    continue; // retry scan
                }
            }

            let shutdown_rx_clone = shutdown_rx.clone();
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    log::info!("Shutdown signal received, exiting main loop.");
                    break;
                }
                res = health_monitor_loop(curl.clone(), server_ip, port, shutdown_rx_clone) => {
                    if res {
                        log::info!("Health monitor stopped due to shutdown.");
                        break;
                    }
                    log::warn!("Health monitor ended: {res}. Rescanning subnet...");
                }
            }
        }
    });

    tokio::signal::ctrl_c().await.unwrap();
    log::info!("Ctrl+C received, starting shutdown...");

    let _ = shutdown_tx.send(());

    web_handler.stop();
    web_handler.handle.await.unwrap();

    main_task.await.unwrap();

    log::info!("Shutdown complete.");
    Ok(())
}
