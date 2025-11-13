use std::time::Duration;

use crate::scanner::{Error, SubnetScannerBuilder};

mod scanner;

#[tokio::main]
async fn main() -> Result<(), Error> {
    // You can override IP/netmask/port/timeout here using builder methods
    SubnetScannerBuilder::new()
        .port(5247)
        .timeout(Duration::from_secs(1))
        .scan()
        .await?;

    Ok(())
}
