use std::time::Duration;

use async_curl::CurlActor;
use curl_http_client::{Collector, HttpClient};
use http::{Method, Request, header};

use crate::scanner::{Error, SubnetScannerBuilder};

mod scanner;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let curl = CurlActor::new();
    let (server_ip, port, device_ip) = loop {
        match SubnetScannerBuilder::new()
            .port(5247)
            .timeout(Duration::from_secs(1))
            .scan(curl.clone())
            .await
        {
            Ok((ip, port, device_ip)) => {
                println!("Success! Found server at {}:{}", ip, port);
                break (ip, port, device_ip);
            }
            Err(e) => {
                eprintln!("Scan failed: {e}. Retrying in 5 second...");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    };

    let url = format!("https://{server_ip}:{port}/register_device");
    println!("{url}");

    let collector = Collector::RamAndHeaders(Vec::new(), Vec::new());
    let body = serde_json::json!({
        "device_ip": device_ip
    });

    let request = Request::builder()
        .uri(url.as_str())
        .method(Method::POST)
        .header(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        )
        .body(Some(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let response = HttpClient::new(collector)
        .request(request)?
        .ssl_verify_host(false)?
        .ssl_verify_peer(false)?
        .nonblocking(curl)
        .perform()
        .await?;

    println!("{:#?}", response.headers());
    println!("{response:#?}");
    Ok(())
}
