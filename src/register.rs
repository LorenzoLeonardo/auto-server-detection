use std::net::Ipv4Addr;

use async_curl::CurlActor;
use curl_http_client::{Collector, HttpClient};
use http::{Method, Request, StatusCode, header};
use serde::Serialize;

use crate::scanner::Error;

pub(crate) async fn register_device(
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
