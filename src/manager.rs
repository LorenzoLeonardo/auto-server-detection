use std::net::Ipv4Addr;

use async_curl::CurlActor;
use curl_http_client::Collector;
use tokio::sync::mpsc;
use tokio::sync::watch;

use crate::error::Error;
use crate::health;
use crate::register;
use crate::scanner::SubnetScannerBuilder;

#[derive(Debug)]
pub enum ManagerMsg {
    ScanResult(Ipv4Addr, u16, Ipv4Addr),
    RegistrationSuccess,
    RegistrationFailed(Error),
    ServerAlive,
    ServerDead,
    Shutdown,
}

pub struct ManagerHandler {
    pub handle: tokio::task::JoinHandle<()>,
    shutdown_tx: watch::Sender<bool>,
}

impl ManagerHandler {
    pub fn stop(&self) {
        let _ = self.shutdown_tx.send(true);
    }
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

    pub async fn run(mut self) -> ManagerHandler {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(async move {
            log::info!("[manager] Manager started");
            loop {
                self.spawn_scan_task(Some(shutdown_rx.clone())).await;

                let (server_ip, port, device_ip) = match self.wait_for_scan().await {
                    Some(v) => v,
                    None => break,
                };

                self.spawn_register_task(server_ip, port, device_ip).await;

                if !self.wait_for_registration().await {
                    continue;
                }

                self.spawn_health_monitor(server_ip, port, Some(shutdown_rx.clone()))
                    .await;

                match self.wait_for_health_events().await {
                    HealthEvent::Shutdown => break,
                    HealthEvent::ServerDead => {
                        log::warn!("[manager] Server died → restarting scan & registration");
                        continue;
                    }
                }
            }
            log::info!("[manager] Manager ended");
        });
        ManagerHandler {
            handle,
            shutdown_tx,
        }
    }

    // ----------------------------------------------------------------------

    async fn spawn_scan_task(&self, shutdown_rx: Option<watch::Receiver<bool>>) {
        let tx = self.tx.clone();
        let curl = self.curl.clone();
        let shutdown_rx = shutdown_rx.clone();
        log::info!("[manager] Starting subnet scan task...");
        loop {
            match SubnetScannerBuilder::new()
                .port(5247)
                .timeout(std::time::Duration::from_secs(1))
                .scan(curl.clone(), shutdown_rx.clone())
                .await
            {
                Ok((sip, port, dip)) => {
                    let _ = tx.send(ManagerMsg::ScanResult(sip, port, dip)).await;
                    break;
                }
                Err(e) => {
                    if let Error::Shutdown(e) = e {
                        log::info!("[manager] {e}");
                        let _ = tx.send(ManagerMsg::Shutdown).await;
                        break;
                    }
                    log::error!("[manager] Scan failed: {e}. Retrying in 5 seconds...");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn wait_for_scan(&mut self) -> Option<(Ipv4Addr, u16, Ipv4Addr)> {
        while let Some(msg) = self.rx.recv().await {
            if let ManagerMsg::ScanResult(s, p, d) = msg {
                return Some((s, p, d));
            }

            if let ManagerMsg::Shutdown = msg {
                log::info!("[manager] Manager shutdown requested");
                return None;
            }
        }
        None
    }

    // ----------------------------------------------------------------------

    async fn spawn_register_task(&self, server_ip: Ipv4Addr, port: u16, device_ip: Ipv4Addr) {
        let tx = self.tx.clone();
        let curl = self.curl.clone();

        match register::register_device(curl, server_ip, port, device_ip).await {
            Ok(_) => {
                let _ = tx.send(ManagerMsg::RegistrationSuccess).await;
            }
            Err(e) => {
                let _ = tx.send(ManagerMsg::RegistrationFailed(e)).await;
            }
        }
    }

    async fn wait_for_registration(&mut self) -> bool {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ManagerMsg::RegistrationSuccess => {
                    log::info!("[manager] Registration succeeded");
                    return true;
                }
                ManagerMsg::RegistrationFailed(e) => {
                    log::error!("[manager] Registration failed: {e}");
                    return false;
                }
                _ => continue,
            }
        }
        false
    }

    // ----------------------------------------------------------------------

    async fn spawn_health_monitor(
        &self,
        server_ip: Ipv4Addr,
        port: u16,
        shutdown_rx: Option<watch::Receiver<bool>>,
    ) {
        let tx = self.tx.clone();
        let curl = self.curl.clone();

        loop {
            tokio::select! {
                // 🔥 Shutdown request received: stop immediately
                _ = async {
                    if let Some(mut rx) = shutdown_rx.clone() {
                        rx.changed().await.ok();
                    }
                }, if shutdown_rx.is_some() => {
                    log::info!("Health monitor received shutdown signal");
                    let _ = tx.send(ManagerMsg::Shutdown).await;
                    return;
                }

                // 🩺 Health check branch
                res = health::health_check(&curl, server_ip, port) => {
                    match res {
                        Ok(_) => {
                            let _ = tx.send(ManagerMsg::ServerAlive).await;
                        }
                        Err(e) => {
                            log::warn!("Health check failed: {e}");
                            let _ = tx.send(ManagerMsg::ServerDead).await;
                            return; // stop monitor on failure
                        }
                    }

                    // Delay for next loop
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn wait_for_health_events(&mut self) -> HealthEvent {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ManagerMsg::ServerDead => return HealthEvent::ServerDead,
                ManagerMsg::Shutdown => break,
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
