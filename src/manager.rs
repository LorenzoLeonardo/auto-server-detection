use std::net::Ipv4Addr;

use curl_http_client::{Collector, dep::async_curl::CurlActor};
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
    curl: CurlActor<Collector>,
}

impl Manager {
    pub fn new(curl: CurlActor<Collector>) -> Self {
        Self { curl }
    }

    pub async fn run(self) -> ManagerHandler {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(async move {
            log::info!("[manager] Manager started");
            loop {
                let (server_ip, port, device_ip) =
                    match self.spawn_scan_task(Some(shutdown_rx.clone())).await {
                        ManagerMsg::ScanResult(server_ip, port, device_ip) => {
                            (server_ip, port, device_ip)
                        }
                        ManagerMsg::Shutdown => {
                            break;
                        }
                        _ => unimplemented!("[manager] scan task has only two return values."),
                    };

                match self.spawn_register_task(server_ip, port, device_ip).await {
                    ManagerMsg::RegistrationSuccess => {}
                    ManagerMsg::RegistrationFailed(error) => {
                        log::error!("[manager] ManagerMsg::RegistrationFailed: {error}");
                        continue;
                    }
                    _ => unimplemented!("[manager] register task has only two return values."),
                }

                match self
                    .spawn_health_monitor(server_ip, port, Some(shutdown_rx.clone()))
                    .await
                {
                    ManagerMsg::ServerDead => {
                        log::info!("[manager] Retry scan. . .");
                        continue;
                    }
                    ManagerMsg::Shutdown => {
                        break;
                    }
                    _ => {
                        unimplemented!("[manager] health monitor task has only two return values.")
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

    async fn spawn_scan_task(&self, shutdown_rx: Option<watch::Receiver<bool>>) -> ManagerMsg {
        let curl = self.curl.clone();

        log::info!("[manager] Starting subnet scan task...");
        loop {
            tokio::select! {
                // 🔹 Shutdown signal received → exit immediately
                _ = async {
                    if let Some(mut rx) = shutdown_rx.clone() {
                        // Wait until it changes to true
                        rx.changed().await.ok();
                    }
                } => {
                    log::info!("[manager] Received shutdown during scan.");
                    return ManagerMsg::Shutdown;
                }

                // 🔹 Run the scanner
                res = SubnetScannerBuilder::new()
                    .port(5247)
                    .timeout(std::time::Duration::from_secs(1))
                    .scan(curl.clone(), shutdown_rx.clone()) => {

                    match res {
                        Ok((sip, port, dip)) => {
                            return ManagerMsg::ScanResult(sip, port, dip);
                        }
                        Err(e) => {
                            if let Error::Shutdown(e) = e {
                                log::info!("[manager] {e}");
                                return ManagerMsg::Shutdown;
                            }
                            log::error!("[manager] Scan failed: {e}. Retrying in 5 seconds...");
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        }
    }
    // ----------------------------------------------------------------------

    async fn spawn_register_task(
        &self,
        server_ip: Ipv4Addr,
        port: u16,
        device_ip: Ipv4Addr,
    ) -> ManagerMsg {
        let curl = self.curl.clone();

        match register::register_device(curl, server_ip, port, device_ip).await {
            Ok(_) => ManagerMsg::RegistrationSuccess,
            Err(e) => ManagerMsg::RegistrationFailed(e),
        }
    }

    // ----------------------------------------------------------------------

    async fn spawn_health_monitor(
        &self,
        server_ip: Ipv4Addr,
        port: u16,
        shutdown_rx: Option<watch::Receiver<bool>>,
    ) -> ManagerMsg {
        let curl = self.curl.clone();

        loop {
            tokio::select! {
                // 🔥 Shutdown request received: stop immediately
                _ = async {
                    if let Some(mut rx) = shutdown_rx.clone() {
                        // Wait until it changes to true
                        rx.changed().await.ok();
                    }
                } => {
                    log::info!("[manager] Received shutdown during health monitor.");
                    return ManagerMsg::Shutdown;
                }

                // 🩺 Health check branch
                res = health::health_check(&curl, server_ip, port) => {
                    match res {
                        Ok(_) => {
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            continue;
                        }
                        Err(e) => {
                            log::warn!("Health check failed: {e}");
                            return ManagerMsg::ServerDead;
                        }
                    }
                }
            }
        }
    }
}
