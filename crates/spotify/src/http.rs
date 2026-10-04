//! HTTP seam. Everything network-shaped goes through [`Transport`] so
//! unit tests script responses with [`FakeTransport`] and never touch
//! the network. The real implementation is one shared `ureq::Agent`
//! with timeouts, returning every status (Spotify's 204/4xx answers are
//! data, not transport errors).

use std::sync::Mutex;

/// One outgoing HTTP request.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// Raw body bytes; `None` for bodiless requests.
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }
}

/// One HTTP answer, whatever the status: error mapping (429 pause, 401
/// refresh, 403/404 reasons) is the caller's job.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

/// Transport failure: the request never produced an HTTP answer.
#[derive(Debug, thiserror::Error)]
#[error("transport: {0}")]
pub struct TransportError(pub String);

pub trait Transport: Send + Sync {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, TransportError>;
}

/// The real transport: one agent, 30 s per call so a wedged network
/// cannot pin an action thread (same bound as the discord crate).
#[derive(Debug)]
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> UreqTransport {
        UreqTransport {
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(Some(std::time::Duration::from_secs(30)))
                .build()
                .new_agent(),
        }
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for UreqTransport {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let method = req.method.as_str();
        // ureq's builder is typestated: bodied methods (post/put) can
        // send, bodiless (get/delete) can call
        let response = match req.method {
            Method::Get => {
                let mut b = self.agent.get(&req.url);
                for (name, value) in &req.headers {
                    b = b.header(name, value);
                }
                b.call()
            }
            Method::Delete => {
                let mut b = self.agent.delete(&req.url);
                for (name, value) in &req.headers {
                    b = b.header(name, value);
                }
                b.call()
            }
            Method::Post | Method::Put => {
                let mut b = match req.method {
                    Method::Post => self.agent.post(&req.url),
                    _ => self.agent.put(&req.url),
                };
                for (name, value) in &req.headers {
                    b = b.header(name, value);
                }
                match &req.body {
                    Some(bytes) => b.send(bytes.as_slice()),
                    None => b.send_empty(),
                }
            }
        };
        let resp = response.map_err(|e| TransportError(format!("{method} {e}")))?;
        let status = resp.status().as_u16();
        let mut headers = Vec::new();
        for (name, value) in resp.headers().iter() {
            // multi-value headers collapse to the last value; only
            // Retry-After is consumed and it is single-valued
            headers.push((
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            ));
        }
        let mut body = Vec::new();
        use std::io::Read as _;
        resp.into_body()
            .into_reader()
            .read_to_end(&mut body)
            .map_err(|e| TransportError(format!("body: {e}")))?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

/// Scripted transport for tests: requests are recorded, answers are
/// played back in order (an `HttpResponse` per request; run out and the
/// call panics, so a test never silently talks to the real network).
#[derive(Default)]
pub struct FakeTransport {
    requests: Mutex<Vec<HttpRequest>>,
    responses: Mutex<Vec<HttpResponse>>,
}

impl FakeTransport {
    pub fn new() -> FakeTransport {
        FakeTransport::default()
    }

    /// Queue one scripted answer.
    pub fn push(&self, resp: HttpResponse) -> &Self {
        self.responses.lock().unwrap().push(resp);
        self
    }

    /// Queue a JSON answer with a status.
    pub fn push_json(&self, status: u16, body: serde_json::Value) -> &Self {
        self.push(HttpResponse {
            status,
            headers: Vec::new(),
            body: serde_json::to_vec(&body).unwrap(),
        })
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn last_url(&self) -> String {
        self.requests()
            .last()
            .map(|r| r.url.clone())
            .expect("a request was made")
    }
}

impl Transport for FakeTransport {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.lock().unwrap().push(req.clone());
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            return Err(TransportError(
                "fake transport has no scripted response".into(),
            ));
        }
        Ok(responses.remove(0))
    }
}
