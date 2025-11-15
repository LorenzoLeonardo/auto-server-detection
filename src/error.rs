use curl_http_client::Collector;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Curl error: {0}")]
    Curl(curl_http_client::Error<Collector>),
    #[error("Http error: {0}")]
    Http(http::Error),
    #[error("Serde error: {0}")]
    Serde(serde_json::Error),
    #[error("Shutdown error: {0}")]
    Shutdown(String),
    #[error("Other error: {0}")]
    Other(String),
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

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Serde(value)
    }
}
