use std::net::IpAddr;
use std::time::Duration;

use async_curl::CurlActor;
use curl_http_client::*;
use futures::stream::{FuturesUnordered, StreamExt};
use http::{Method, Request};
use tokio::net::TcpStream;
use tokio::time;

#[derive(thiserror::Error, Debug)]
enum Error {
    #[error("Curl error: {0}")]
    Curl(curl_http_client::Error<Collector>),
    #[error("Http error: {0}")]
    Http(http::Error),
}

impl From<curl_http_client::Error<Collector>> for Error {
    fn from(value: curl_http_client::Error<Collector>) -> Self {
        Self::Curl(value)
    }
}

impl From<http::Error> for Error {
    fn from(value: http::Error) -> Self {
        Self::Http(value)
    }
}

/// Try to connect and confirm an HTTP response
async fn check_http_server(
    ip: IpAddr,
    port: u16,
    actor: CurlActor<Collector>,
) -> Result<Option<IpAddr>, Error> {
    let addr = format!("{}:{}", ip, port);
    let timeout = Duration::from_secs(1);
    // Quick TCP check first
    if time::timeout(timeout, TcpStream::connect(&addr))
        .await
        .is_err()
    {
        return Ok(None);
    }
    let url = format!("https://{addr}");
    let collector = Collector::Ram(Vec::new());
    let request = Request::builder()
        .uri(url.as_str())
        .method(Method::GET)
        .body(None)?;

    let result = HttpClient::new(collector)
        .request(request)?
        .connect_timeout(timeout)?
        .ssl_verify_host(false)?
        .ssl_verify_peer(false)?
        .nonblocking(actor)
        .perform()
        .await?;

    println!(
        "✅ Found HTTP server at {} (status: {})",
        ip,
        result.status()
    );
    Ok(Some(ip))
}

#[tokio::main]
async fn main() {
    let port = 5247;

    // 🔍 Try to guess your subnet automatically
    // For now, default to common private ranges
    let subnets = ["192.168.0."];

    let actor = CurlActor::new();

    for subnet in subnets {
        println!(
            "🔎 Scanning subnet {}0/24 for HTTPS servers on port {}...",
            subnet, port
        );

        let mut tasks = FuturesUnordered::new();
        for i in 1..=254 {
            let ip: IpAddr = format!("{}{}", subnet, i).parse().unwrap();
            let c = actor.clone();
            tasks.push(tokio::spawn(
                async move { check_http_server(ip, port, c).await },
            ));
        }

        let mut found = false;
        while let Some(res) = tasks.next().await {
            if let Ok(Ok(Some(ip))) = res {
                println!("🎯 Active HTTP server detected at {}", ip);
                found = true;
            }
        }

        if found {
            println!(
                "✅ Finished scanning {}0/24 — found active server(s).",
                subnet
            );
        } else {
            println!("❌ No HTTP servers found in {}0/24.", subnet);
        }
    }
}
