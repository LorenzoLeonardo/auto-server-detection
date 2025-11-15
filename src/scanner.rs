use std::net::Ipv4Addr;
use std::time::Duration;

use async_curl::CurlActor;
use curl_http_client::*;
use futures::stream::{FuturesUnordered, StreamExt};
use http::{Method, Request};
use if_addrs::{IfAddr, get_if_addrs};
use local_ip_address::local_ip;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::time;

use crate::error::Error;

async fn check_http_server(
    ip: Ipv4Addr,
    port: u16,
    actor: CurlActor<Collector>,
    timeout: Duration,
) -> Result<Option<Ipv4Addr>, Error> {
    let addr = format!("{}:{}", ip, port);

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

    log::info!(
        "[scanner] ✅ Found HTTP server at {} (status: {})",
        ip,
        result.status()
    );
    Ok(Some(ip))
}

// Helpers
fn netmask_to_prefixlen(netmask: Ipv4Addr) -> u32 {
    netmask.octets().iter().map(|b| b.count_ones()).sum()
}

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

fn ip_range(network: Ipv4Addr, prefix_len: u32) -> impl Iterator<Item = Ipv4Addr> {
    if prefix_len > 32 {
        panic!("[scanner] Invalid prefix length: {}", prefix_len);
    }

    let host_bits = 32 - prefix_len;

    // Use u64 to safely compute number of hosts
    let num_hosts = if host_bits == 0 {
        0
    } else {
        2u64.pow(host_bits) - 2 // exclude network & broadcast
    };

    let base = u32::from(network) as u64;

    (1..=num_hosts).map(move |i| Ipv4Addr::from((base + i) as u32))
}

// Builder struct for subnet scanning
pub struct SubnetScannerBuilder {
    ip: Option<Ipv4Addr>,
    netmask: Option<Ipv4Addr>,
    port: u16,
    timeout: Duration,
}

impl SubnetScannerBuilder {
    pub fn new() -> Self {
        Self {
            ip: None,
            netmask: None,
            port: 0,
            timeout: Duration::from_secs(0),
        }
    }

    /// Set Port
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set TCP connect and HTTP timeout duration
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Auto-detect local IP and netmask if not set
    fn auto_detect_network(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.ip.is_some() && self.netmask.is_some() {
            return Ok(());
        }

        let local_ip = local_ip()?;
        let iface = get_if_addrs()?
            .into_iter()
            .find(|iface| iface.addr.ip() == local_ip)
            .ok_or("[scanner] No matching interface for local IP found")?;

        let (ip, netmask) = match iface.addr {
            IfAddr::V4(mask) => (mask.ip, mask.netmask),
            _ => return Err("[scanner] No IPv4 netmask found".into()),
        };

        self.ip.get_or_insert(ip);
        self.netmask.get_or_insert(netmask);

        Ok(())
    }

    pub async fn scan(
        self,
        curl_actor: CurlActor<Collector>,
        mut shutdown_rx: Option<watch::Receiver<bool>>,
    ) -> Result<(Ipv4Addr, u16, Ipv4Addr), Error> {
        let mut builder = self;
        builder
            .auto_detect_network()
            .map_err(|e| Error::Other(format!("[scanner] Failed to auto detect network: {}", e)))?;

        let local_ip = builder.ip.unwrap();
        let netmask = builder.netmask.unwrap();
        let prefix_len = netmask_to_prefixlen(netmask);
        let network = network_address(local_ip, netmask);
        let ips: Vec<Ipv4Addr> = ip_range(network, prefix_len).collect();

        log::info!("[scanner] Local IP detected: {}", local_ip);
        log::info!("[scanner] Netmask: {}", netmask);
        log::info!("[scanner] Prefix length: /{}", prefix_len);
        log::info!("[scanner] Network address: {}", network);
        log::info!(
            "[scanner] 🔎 Scanning subnet {}/{} for HTTPS servers on port {}...",
            network,
            prefix_len,
            builder.port
        );
        let mut tasks = FuturesUnordered::new();
        let timeout = builder.timeout;
        let port = builder.port;

        for ip in ips {
            let c = curl_actor.clone();
            tasks.push(tokio::spawn(async move {
                check_http_server(ip, port, c, timeout).await
            }));
        }

        while let Some(res) = if let Some(ref mut rx) = shutdown_rx {
            tokio::select! {
                _ = rx.changed() => {
                    log::info!("[scanner] Scanner shutdown requested");
                    return Err(Error::Shutdown("[scanner] Scanner shutdown".into()));
                }
                res = tasks.next() => res,
            }
        } else {
            tasks.next().await
        } {
            if let Ok(Ok(Some(ip))) = res {
                log::info!("[scanner] 🎯 Active HTTP server detected at {}", ip);
                return Ok((ip, port, local_ip));
            }
        }
        Err(Error::Other(format!(
            "[scanner] ❌ No HTTP servers found in {network}/{prefix_len}."
        )))
    }
}
