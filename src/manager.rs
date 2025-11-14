use std::net::Ipv4Addr;

use async_curl::CurlActor;
use curl_http_client::Collector;
use tokio::sync::mpsc;

use crate::health_check;
use crate::register;
use crate::scanner::Error;
use crate::scanner::SubnetScannerBuilder;

#[derive(Debug)]
pub enum ManagerMsg {
    ScanResult(Ipv4Addr, u16, Ipv4Addr),
    RegistrationSuccess,
    RegistrationFailed(Error),
    ServerAlive,
    ServerDead,
}

pub struct Manager {
    tx: mpsc::Sender<ManagerMsg>,
    rx: mpsc::Receiver<ManagerMsg>,
    curl: CurlActor<Collector>,
}

impl Manager {
    pub fn new(curl: CurlActor<Collector>) -> Self {
        let (tx, rx) = mpsc::channel(32);
        Self { tx, rx, curl }
    }

    pub async fn run(mut self) {
        loop {
            self.spawn_scan_task();

            let (server_ip, port, device_ip) = match self.wait_for_scan().await {
                Some(v) => v,
                None => continue,
            };

            self.spawn_register_task(server_ip, port, device_ip);

            if !self.wait_for_registration().await {
                continue;
            }

            self.spawn_health_monitor(server_ip, port);

            match self.wait_for_health_events().await {
                HealthEvent::Shutdown => break,
                HealthEvent::ServerDead => {
                    log::warn!("Server died → restarting scan & registration");
                    continue;
                }
            }
        }
    }

    // ----------------------------------------------------------------------

    fn spawn_scan_task(&self) {
        let tx = self.tx.clone();
        let curl = self.curl.clone();

        tokio::spawn(async move {
            loop {
                match SubnetScannerBuilder::new()
                    .port(5247)
                    .timeout(std::time::Duration::from_secs(1))
                    .scan(curl.clone())
                    .await
                {
                    Ok((sip, port, dip)) => {
                        let _ = tx.send(ManagerMsg::ScanResult(sip, port, dip)).await;
                        break;
                    }
                    Err(e) => {
                        log::error!("Scan failed: {e}. Retrying in 5 seconds...");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });
    }

    async fn wait_for_scan(&mut self) -> Option<(Ipv4Addr, u16, Ipv4Addr)> {
        while let Some(msg) = self.rx.recv().await {
            if let ManagerMsg::ScanResult(s, p, d) = msg {
                return Some((s, p, d));
            }
        }
        None
    }

    // ----------------------------------------------------------------------

    fn spawn_register_task(&self, server_ip: Ipv4Addr, port: u16, device_ip: Ipv4Addr) {
        let tx = self.tx.clone();
        let curl = self.curl.clone();

        tokio::spawn(async move {
            match register::register_device(curl, server_ip, port, device_ip).await {
                Ok(_) => {
                    let _ = tx.send(ManagerMsg::RegistrationSuccess).await;
                }
                Err(e) => {
                    let _ = tx.send(ManagerMsg::RegistrationFailed(e)).await;
                }
            }
        });
    }

    async fn wait_for_registration(&mut self) -> bool {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ManagerMsg::RegistrationSuccess => {
                    log::info!("Registration succeeded");
                    return true;
                }
                ManagerMsg::RegistrationFailed(e) => {
                    log::error!("Registration failed: {e}");
                    return false;
                }
                _ => continue,
            }
        }
        false
    }

    // ----------------------------------------------------------------------

    fn spawn_health_monitor(&self, server_ip: Ipv4Addr, port: u16) {
        let tx = self.tx.clone();
        let curl = self.curl.clone();

        tokio::spawn(async move {
            loop {
                match health_check(&curl, server_ip, port).await {
                    Ok(_) => {
                        let _ = tx.send(ManagerMsg::ServerAlive).await;
                    }
                    Err(e) => {
                        log::warn!("Health check failed: {e}");
                        let _ = tx.send(ManagerMsg::ServerDead).await;
                        return; // stop monitor
                    }
                }

                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
    }

    async fn wait_for_health_events(&mut self) -> HealthEvent {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ManagerMsg::ServerDead => return HealthEvent::ServerDead,
                _ => continue,
            }
        }
        HealthEvent::Shutdown
    }
}

pub enum HealthEvent {
    ServerDead,
    Shutdown,
}
