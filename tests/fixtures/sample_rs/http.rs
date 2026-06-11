/// HTTP client abstraction with retry logic.

use std::time::Duration;
use std::fmt;

/// HTTP method enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Method::Get => write!(f, "GET"),
            Method::Post => write!(f, "POST"),
            Method::Put => write!(f, "PUT"),
            Method::Delete => write!(f, "DELETE"),
            Method::Patch => write!(f, "PATCH"),
        }
    }
}

/// HTTP response status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusCode(pub u16);

impl StatusCode {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.0)
    }

    pub fn is_client_error(&self) -> bool {
        (400..500).contains(&self.0)
    }

    pub fn is_server_error(&self) -> bool {
        (500..600).contains(&self.0)
    }
}

/// HTTP request.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl Request {
    pub fn new(method: Method, url: &str) -> Self {
        Self {
            method,
            url: url.to_string(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn with_header(mut self, key: &str, value: &str) -> Self {
        self.headers.push((key.to_string(), value.to_string()));
        self
    }

    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }
}

/// HTTP response.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: StatusCode,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn body_as_string(&self) -> Result<String, std::string::FromUtf8Error> {
        String::from_utf8(self.body.clone())
    }
}

/// HTTP client trait.
pub trait HttpClient: Send + Sync {
    fn send(&self, request: &Request) -> Result<Response, String>;
}

/// Retry configuration.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(5),
            multiplier: 2.0,
        }
    }
}

/// HTTP client with retry logic.
pub struct RetryClient<C: HttpClient> {
    inner: C,
    config: RetryConfig,
}

impl<C: HttpClient> RetryClient<C> {
    pub fn new(inner: C, config: RetryConfig) -> Self {
        Self { inner, config }
    }

    pub fn send(&self, request: &Request) -> Result<Response, String> {
        let mut attempt = 0;
        let mut delay = self.config.initial_delay;

        loop {
            attempt += 1;
            match self.inner.send(request) {
                Ok(response) if response.status.is_success() => return Ok(response),
                Ok(response) if response.status.is_client_error() => {
                    return Err(format!("Client error: {}", response.status.0));
                }
                Ok(_) if attempt >= self.config.max_attempts => {
                    return Err(format!("Max attempts ({}) exceeded", self.config.max_attempts));
                }
                Ok(_) => {
                    std::thread::sleep(delay);
                    delay = std::cmp::min(
                        Duration::from_millis((delay.as_millis() as f64 * self.config.multiplier) as u64),
                        self.config.max_delay,
                    );
                }
                Err(e) if attempt >= self.config.max_attempts => {
                    return Err(format!("Request failed after {} attempts: {}", self.config.max_attempts, e));
                }
                Err(_) => {
                    std::thread::sleep(delay);
                    delay = std::cmp::min(
                        Duration::from_millis((delay.as_millis() as f64 * self.config.multiplier) as u64),
                        self.config.max_delay,
                    );
                }
            }
        }
    }
}
