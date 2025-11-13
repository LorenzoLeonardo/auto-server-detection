use std::net::Ipv4Addr;
use std::time::Duration;

use async_curl::CurlActor;
use curl_http_client::*;
use futures::stream::{FuturesUnordered, StreamExt};
use http::{Method, Request};
use if_addrs::{IfAddr, get_if_addrs};
use local_ip_address::local_ip;
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
    ip: Ipv4Addr,
    port: u16,
    actor: CurlActor<Collector>,
) -> Result<Option<Ipv4Addr>, Error> {
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

// Helper: convert netmask (e.g. 255.255.255.0) to prefix length (e.g. 24)
fn netmask_to_prefixlen(netmask: Ipv4Addr) -> u32 {
    let octets = netmask.octets();
    let mut bits = 0;
    for byte in octets {
        bits += byte.count_ones();
    }
    bits
}

// Helper: calculate network address (IP & mask)
fn network_address(ip: Ipv4Addr, netmask: Ipv4Addr) -> Ipv4Addr {
    let ip_octets = ip.octets();
    let mask_octets = netmask.octets();
    Ipv4Addr::new(
        ip_octets[0] & mask_octets[0],
        ip_octets[1] & mask_octets[1],
        ip_octets[2] & mask_octets[2],
        ip_octets[3] & mask_octets[3],
    )
}

// Helper: generate IP range iterator over subnet
fn ip_range(network: Ipv4Addr, prefix_len: u32) -> impl Iterator<Item = Ipv4Addr> {
    let host_bits = 32 - prefix_len;
    let num_hosts = 2u32.pow(host_bits) - 2; // exclude network & broadcast

    let base = u32::from(network);

    (1..=num_hosts).map(move |i| Ipv4Addr::from(base + i))
}

#[tokio::main]
async fn main() {
    let port = 5247;

    // 🔍 Try to guess your subnet automatically
    // For now, default to common private ranges
    let local_ip = local_ip().expect("Failed to get local IP");
    println!("Local IP detected: {}", local_ip);

    let iface = get_if_addrs()
        .expect("Failed to get interfaces")
        .into_iter()
        .find(|iface| iface.addr.ip() == local_ip)
        .expect("No IPv4 interface found");

    let netmask = match iface.addr {
        IfAddr::V4(mask) => mask,
        _ => panic!("No IPv4 netmask found"),
    };
    let local_ip = netmask.ip;
    let netmask = netmask.netmask;

    println!("Local IP : {local_ip} Mask: {netmask}");

    let prefix_len = netmask_to_prefixlen(netmask);
    println!("Prefix length: /{}", prefix_len);

    let network = network_address(local_ip, netmask);
    println!("Network address: {}", network);

    let ips: Vec<Ipv4Addr> = ip_range(network, prefix_len).collect();

    let actor = CurlActor::new();

    println!("🔎 Scanning subnet {network}/{prefix_len} for HTTPS servers on port {port}...");

    let mut tasks = FuturesUnordered::new();
    for ip in ips {
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
        println!("✅ Finished scanning {network}/{prefix_len} — found active server(s).",);
    } else {
        println!("❌ No HTTP servers found in {network}/{prefix_len}.");
    }
}
