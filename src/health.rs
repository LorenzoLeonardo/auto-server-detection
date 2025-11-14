use std::{net::Ipv4Addr, time::Duration};

use anyhow::Result;
use async_curl::CurlActor;
use curl_http_client::{Collector, HttpClient};
use http::{Method, Request, StatusCode};

use crate::scanner::Error;

pub(crate) async fn health_check(
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
