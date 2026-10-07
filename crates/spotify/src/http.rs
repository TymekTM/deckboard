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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> HttpResponse {
        HttpResponse {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.to_vec(),
        }
    }

    #[test]
    fn method_names() {
        assert_eq!(Method::Get.as_str(), "GET");
        assert_eq!(Method::Post.as_str(), "POST");
        assert_eq!(Method::Put.as_str(), "PUT");
        assert_eq!(Method::Delete.as_str(), "DELETE");
    }

    #[test]
    fn header_lookup_is_case_insensitive_and_first_wins() {
        let r = response(429, &[("Retry-After", "3"), ("retry-after", "9")], b"");
        assert_eq!(r.header("retry-after"), Some("3"));
        assert_eq!(r.header("RETRY-AFTER"), Some("3"));
        assert_eq!(r.header("x-missing"), None);
    }

    #[test]
    fn json_body_parses_or_reads_as_none() {
        assert_eq!(
            response(200, &[], br#"{"a":1}"#).json(),
            Some(serde_json::json!({"a": 1}))
        );
        assert_eq!(response(204, &[], b"").json(), None);
        assert_eq!(response(502, &[], b"<html>").json(), None);
    }

    #[test]
    fn transport_error_display() {
        assert_eq!(
            TransportError("GET timeout".into()).to_string(),
            "transport: GET timeout"
        );
    }

    fn get(url: &str) -> HttpRequest {
        HttpRequest {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    #[test]
    fn fake_transport_records_and_replays_in_order() {
        let fake = FakeTransport::new();
        fake.push_json(200, serde_json::json!({"n": 1}))
            .push(response(204, &[], b""));
        let first = fake.send(&get("https://a/1")).unwrap();
        let second = fake.send(&get("https://a/2")).unwrap();
        assert_eq!(first.status, 200);
        assert_eq!(first.json().unwrap()["n"], 1);
        assert_eq!(second.status, 204);
        assert_eq!(fake.last_url(), "https://a/2");
        assert_eq!(
            fake.requests()
                .iter()
                .map(|r| r.url.as_str())
                .collect::<Vec<_>>(),
            vec!["https://a/1", "https://a/2"]
        );
    }

    #[test]
    fn fake_transport_fails_when_the_script_runs_out() {
        let fake = FakeTransport::new();
        let err = fake.send(&get("https://a/")).unwrap_err();
        assert!(err.0.contains("no scripted response"));
        // the attempt is still recorded
        assert_eq!(fake.requests().len(), 1);
    }

    #[test]
    #[should_panic(expected = "a request was made")]
    fn last_url_without_requests_panics() {
        FakeTransport::new().last_url();
    }

    /// What the one-shot local server saw.
    struct Seen {
        request_line: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    /// Serves exactly one request with the canned raw `reply`.
    fn one_shot(reply: &'static str) -> (String, std::thread::JoinHandle<Seen>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap();
                headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
            }
            let len = headers
                .iter()
                .find(|(k, _)| k == "content-length")
                .map(|(_, v)| v.parse::<usize>().unwrap())
                .unwrap_or(0);
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            stream.write_all(reply.as_bytes()).unwrap();
            stream.flush().unwrap();
            Seen {
                request_line: request_line.trim_end().to_string(),
                headers,
                body,
            }
        });
        (format!("http://{addr}"), handle)
    }

    #[test]
    fn ureq_transport_sends_headers_and_body() {
        let (base, server) = one_shot(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
        );
        let resp = UreqTransport::new()
            .send(&HttpRequest {
                method: Method::Put,
                url: format!("{base}/v1/me/player/volume?volume_percent=40"),
                headers: vec![("Authorization".into(), "Bearer tok".into())],
                body: Some(b"{}".to_vec()),
            })
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("content-type"), Some("application/json"));
        assert_eq!(resp.json().unwrap()["ok"], true);
        let seen = server.join().unwrap();
        assert_eq!(
            seen.request_line,
            "PUT /v1/me/player/volume?volume_percent=40 HTTP/1.1"
        );
        assert!(seen
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer tok"));
        assert_eq!(seen.body, b"{}");
    }

    #[test]
    fn ureq_transport_returns_error_statuses_as_data() {
        let (base, server) = one_shot(
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 4\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let resp = UreqTransport::default()
            .send(&get(&format!("{base}/x")))
            .unwrap();
        assert_eq!(resp.status, 429);
        assert_eq!(resp.header("retry-after"), Some("4"));
        assert!(resp.body.is_empty());
        assert!(server.join().unwrap().request_line.starts_with("GET /x "));
    }

    #[test]
    fn ureq_transport_sends_bodiless_post_and_delete() {
        for method in [Method::Post, Method::Delete] {
            let (base, server) = one_shot("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
            let resp = UreqTransport::new()
                .send(&HttpRequest {
                    method,
                    url: format!("{base}/p"),
                    headers: Vec::new(),
                    body: None,
                })
                .unwrap();
            assert_eq!(resp.status, 204);
            let seen = server.join().unwrap();
            assert!(
                seen.request_line
                    .starts_with(&format!("{} /p ", method.as_str())),
                "{}",
                seen.request_line
            );
            assert!(seen.body.is_empty());
        }
    }

    #[test]
    fn ureq_transport_maps_refused_connections_to_transport_errors() {
        // bind then drop: nothing listens on the port any more
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let err = UreqTransport::new()
            .send(&get(&format!("http://127.0.0.1:{port}/")))
            .unwrap_err();
        assert!(err.0.starts_with("GET "), "{}", err.0);
    }
}
