//! HTTP request action execution, parsing, redaction, and placeholder substitution.

use std::time::{Duration, Instant};

use pulpit_actions::EventSink;

use crate::SqlBackend;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct HttpRequestConfig {
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "default_body_type", alias = "bodyType", alias = "body_type")]
    pub body_type: String,
    #[serde(default = "default_timeout", alias = "timeout_seconds", alias = "timeoutSeconds")]
    pub timeout: Option<u64>,
    #[serde(default, alias = "ignoreCertErrors", alias = "ignore_cert_errors", alias = "insecure")]
    pub ignore_cert_errors: Option<CertErrorOption>,
    #[serde(default, alias = "responseVar", alias = "response_var", alias = "var", alias = "variable")]
    pub response_var: Option<String>,
    #[serde(default, alias = "jsonPath", alias = "json_path", alias = "path")]
    pub json_path: Option<String>,
}

fn default_method() -> String {
    "GET".into()
}

fn default_body_type() -> String {
    "none".into()
}

fn default_timeout() -> Option<u64> {
    Some(10)
}

/// Flexible boolean deserializer accepting bools, strings ("true", "false", "yes", "tak"),
/// and integers (1, 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CertErrorOption(pub bool);

impl<'de> serde::Deserialize<'de> for CertErrorOption {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = CertErrorOption;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a boolean, string, or integer")
            }
            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
                Ok(CertErrorOption(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
                Ok(CertErrorOption(v != 0))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
                Ok(CertErrorOption(v != 0))
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                let lower = v.trim().to_ascii_lowercase();
                Ok(CertErrorOption(
                    lower == "true" || lower == "yes" || lower == "1" || lower == "tak" || lower == "on",
                ))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

/// Parse multiline headers (one `Name: value` per line).
pub fn parse_headers(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((name, val)) = line.split_once(':') {
            let name = name.trim();
            let val = val.trim();
            if !name.is_empty() {
                out.push((name.to_string(), val.to_string()));
            }
        }
    }
    out
}

/// Check if a header name is sensitive and must have its value redacted from logs.
pub fn is_sensitive_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "authorization"
        || lower == "cookie"
        || lower.contains("token")
        || lower.contains("key")
        || lower.contains("secret")
}

/// Redact sensitive header values (`Authorization`, `Cookie`, or containing `token`/`key`/`secret`).
pub fn redact_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, val)| {
            if is_sensitive_header(name) {
                (name.clone(), "[REDACTED]".to_string())
            } else {
                (name.clone(), val.clone())
            }
        })
        .collect()
}

/// RFC 3986 percent-encoding for URI components.
pub fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            other => {
                use std::fmt::Write;
                let _ = write!(out, "%{:02X}", other);
            }
        }
    }
    out
}

/// Substitute `{{var:<key>}}` placeholders with variable values.
/// In URLs, substituted values are URL-encoded; in bodies, they remain raw.
pub fn substitute_placeholders(
    template: &str,
    is_url: bool,
    lookup: impl Fn(&str) -> Option<String>,
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{var:") {
        out.push_str(&rest[..start]);
        let after_start = &rest[start + 6..];
        if let Some(end) = after_start.find("}}") {
            let key = after_start[..end].trim();
            if let Some(val) = lookup(key) {
                if is_url {
                    out.push_str(&url_encode(&val));
                } else {
                    out.push_str(&val);
                }
            }
            rest = &after_start[end + 2..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Extract value from JSON by JSON pointer (`/state` or `/items/0/id`) or dotted path (`data.temp`).
pub fn extract_json_value(json: &serde_json::Value, path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let target = if path.starts_with('/') {
        json.pointer(path)
    } else {
        let mut cur = json;
        let mut found = true;
        for part in path.split('.') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some(obj) = cur.as_object() {
                if let Some(v) = obj.get(part) {
                    cur = v;
                    continue;
                }
            }
            if let Some(arr) = cur.as_array() {
                if let Ok(idx) = part.parse::<usize>() {
                    if let Some(v) = arr.get(idx) {
                        cur = v;
                        continue;
                    }
                }
            }
            found = false;
            break;
        }
        if found {
            Some(cur)
        } else {
            None
        }
    };

    target.map(format_json_value)
}

fn format_json_value(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(crate) fn execute_http_action(
    backend: &SqlBackend,
    cmd: &pulpit_actions::Command,
    sink: &mut dyn EventSink,
) {
    let cfg: HttpRequestConfig = match cmd.command.as_deref() {
        Some(c) => match serde_json::from_str(c) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!(kind = "http-request", error = %e, "Invalid http-request configuration JSON");
                backend.set_last_http_error(format!("Invalid HTTP configuration: {e}"));
                return;
            }
        },
        None => {
            tracing::warn!(kind = "http-request", "Tile has no command configuration");
            backend.set_last_http_error("No HTTP request configuration provided".into());
            return;
        }
    };

    if cfg.url.trim().is_empty() {
        tracing::warn!(kind = "http-request", "Tile has empty URL configured");
        backend.set_last_http_error("No URL configured for HTTP request".into());
        return;
    }

    // a shared reborrow keeps the closure Copy, so it can ride each
    // substitute_placeholders call by value
    let sink_values = &*sink;
    let lookup = move |key: &str| {
        sink_values
            .get_app_value(key)
            .or_else(|| backend.get_custom_value(key))
    };

    let substituted_url = substitute_placeholders(&cfg.url, true, lookup);
    let substituted_body = substitute_placeholders(&cfg.body, false, lookup);

    let mut parsed_headers = parse_headers(&cfg.headers);
    for (_k, v) in &mut parsed_headers {
        *v = substitute_placeholders(v, false, lookup);
    }

    let body_type_lower = cfg.body_type.trim().to_ascii_lowercase();
    let default_content_type = match body_type_lower.as_str() {
        "json" => Some("application/json"),
        "form" => Some("application/x-www-form-urlencoded"),
        "text" => Some("text/plain"),
        _ => None,
    };

    if !parsed_headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    {
        if let Some(ct) = default_content_type {
            parsed_headers.push(("Content-Type".to_string(), ct.to_string()));
        }
    }

    let timeout_secs = cfg.timeout.unwrap_or(10).max(1);
    let ignore_certs = cfg.ignore_cert_errors.map(|o| o.0).unwrap_or(false);
    let agent = pulpit_db::http_agent_insecure(
        Duration::from_secs(timeout_secs),
        false,
        ignore_certs,
    );

    let safe_headers = redact_headers(&parsed_headers);
    tracing::debug!(headers = ?safe_headers, "HTTP request headers");

    let method_upper = cfg.method.trim().to_ascii_uppercase();
    let start_time = Instant::now();

    let result = send_request(
        &agent,
        &method_upper,
        &substituted_url,
        &parsed_headers,
        &substituted_body,
    );
    let duration = start_time.elapsed();

    match result {
        Ok(mut response) => {
            let status = response.status().as_u16();
            tracing::info!(
                method = %method_upper,
                url = %substituted_url,
                status = status,
                duration_ms = duration.as_millis(),
                "HTTP request completed"
            );

            // display tiles only ever show a short label: a runaway
            // endpoint must not stream gigabytes into the press thread
            use std::io::Read as _;
            let mut body_str = String::new();
            let _ = response
                .body_mut()
                .as_reader()
                .take(MAX_RESPONSE_BODY)
                .read_to_string(&mut body_str);

            if let Some(var_key) = cfg
                .response_var
                .as_deref()
                .map(str::trim)
                .filter(|k| !k.is_empty())
            {
                let val_to_publish = if let Some(path) = cfg
                    .json_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                {
                    match serde_json::from_str::<serde_json::Value>(&body_str) {
                        Ok(json) => match extract_json_value(&json, path) {
                            Some(v) => v,
                            None => {
                                tracing::warn!(
                                    path = %path,
                                    "JSON path not found in response body; publishing status code"
                                );
                                status.to_string()
                            }
                        },
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                path = %path,
                                "Response is not valid JSON; publishing status code"
                            );
                            status.to_string()
                        }
                    }
                } else {
                    status.to_string()
                };

                sink.app_value(var_key, &val_to_publish);
                backend.set_custom_value(var_key, &val_to_publish);
            }
        }
        Err(e) => {
            tracing::warn!(
                method = %method_upper,
                url = %substituted_url,
                duration_ms = duration.as_millis(),
                error = %e,
                "HTTP request failed"
            );
            backend.set_last_http_error(format!("HTTP request failed: {e}"));
        }
    }
}

fn send_request(
    agent: &ureq::Agent,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &str,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    match method {
        "POST" => send_body(with_headers(agent.post(url), headers), body),
        "PUT" => send_body(with_headers(agent.put(url), headers), body),
        "PATCH" => send_body(with_headers(agent.patch(url), headers), body),
        "DELETE" => with_headers(agent.delete(url), headers).call(),
        _ => with_headers(agent.get(url), headers).call(),
    }
}

/// Largest response body the action will buffer, in bytes. Everything
/// past this is cut off (the extract-JSON step then falls back to the
/// status code).
const MAX_RESPONSE_BODY: u64 = 512 * 1024;

fn with_headers<B>(
    req: ureq::RequestBuilder<B>,
    headers: &[(String, String)],
) -> ureq::RequestBuilder<B> {
    let mut req = req;
    for (k, v) in headers {
        req = req.header(k, v);
    }
    req
}

fn send_body(
    req: ureq::RequestBuilder<ureq::typestate::WithBody>,
    body: &str,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    if body.is_empty() {
        req.send_empty()
    } else {
        req.send(body.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // the Backend trait brings `exec`/`slider` into scope for the
    // integration tests below
    use pulpit_legacy::Backend;

    #[test]
    fn test_parse_headers() {
        let raw = "Content-Type: application/json\nAuthorization: Bearer secret:key:123\n\n  X-Custom: val  \n";
        let parsed = parse_headers(raw);
        assert_eq!(
            parsed,
            vec![
                ("Content-Type".into(), "application/json".into()),
                ("Authorization".into(), "Bearer secret:key:123".into()),
                ("X-Custom".into(), "val".into()),
            ]
        );
    }

    #[test]
    fn test_redact_headers() {
        let headers = vec![
            ("Content-Type".into(), "application/json".into()),
            ("Authorization".into(), "Bearer supersecret".into()),
            ("Cookie".into(), "session=abc".into()),
            ("X-Api-Key".into(), "my-api-key".into()),
            ("X-Auth-Token".into(), "token123".into()),
            ("Client-Secret".into(), "secret456".into()),
            ("Accept".into(), "*/*".into()),
        ];
        let redacted = redact_headers(&headers);
        assert_eq!(
            redacted,
            vec![
                ("Content-Type".into(), "application/json".into()),
                ("Authorization".into(), "[REDACTED]".into()),
                ("Cookie".into(), "[REDACTED]".into()),
                ("X-Api-Key".into(), "[REDACTED]".into()),
                ("X-Auth-Token".into(), "[REDACTED]".into()),
                ("Client-Secret".into(), "[REDACTED]".into()),
                ("Accept".into(), "*/*".into()),
            ]
        );
    }

    #[test]
    fn test_substitute_placeholders_url() {
        let mut vars: HashMap<&str, String> = HashMap::new();
        vars.insert("device", "living room light".into());
        vars.insert("query", "a&b=c".into());

        let url = "https://ha.local/api/state/{{var:device}}?q={{var:query}}&missing={{var:unset}}";
        let substituted = substitute_placeholders(url, true, |k| vars.get(k).cloned());
        assert_eq!(
            substituted,
            "https://ha.local/api/state/living%20room%20light?q=a%26b%3Dc&missing="
        );
    }

    #[test]
    fn test_substitute_placeholders_body() {
        let mut vars: HashMap<&str, String> = HashMap::new();
        vars.insert("name", "living room light".into());
        vars.insert("state", "on".into());

        let body = r#"{"entity": "{{var:name}}", "state": "{{var:state}}"}"#;
        let substituted = substitute_placeholders(body, false, |k| vars.get(k).cloned());
        assert_eq!(
            substituted,
            r#"{"entity": "living room light", "state": "on"}"#
        );
    }

    #[test]
    fn test_extract_json_value_pointer() {
        let json: serde_json::Value = serde_json::json!({
            "state": "active",
            "items": [
                { "id": 101, "name": "lamp" },
                { "id": 102, "name": "fan" }
            ],
            "nested": {
                "deep": {
                    "count": 42
                }
            }
        });

        assert_eq!(extract_json_value(&json, "/state").as_deref(), Some("active"));
        assert_eq!(extract_json_value(&json, "/items/0/id").as_deref(), Some("101"));
        assert_eq!(extract_json_value(&json, "/items/1/name").as_deref(), Some("fan"));
        assert_eq!(extract_json_value(&json, "/nested/deep/count").as_deref(), Some("42"));
        assert_eq!(extract_json_value(&json, "/missing"), None);
    }

    #[test]
    fn test_extract_json_value_dotted() {
        let json: serde_json::Value = serde_json::json!({
            "data": {
                "temperature": 21.5,
                "humidity": 55,
                "sensors": [
                    { "type": "temp", "val": "22C" }
                ]
            },
            "status": "ok"
        });

        assert_eq!(extract_json_value(&json, "status").as_deref(), Some("ok"));
        assert_eq!(extract_json_value(&json, "data.temperature").as_deref(), Some("21.5"));
        assert_eq!(extract_json_value(&json, "data.sensors.0.val").as_deref(), Some("22C"));
        assert_eq!(extract_json_value(&json, "data.missing"), None);
    }

    #[test]
    fn test_deserialize_cert_error_option() {
        #[derive(serde::Deserialize)]
        struct TestConfig {
            insecure: Option<CertErrorOption>,
        }

        let cfg1: TestConfig = serde_json::from_str(r#"{"insecure": true}"#).unwrap();
        assert_eq!(cfg1.insecure, Some(CertErrorOption(true)));

        let cfg2: TestConfig = serde_json::from_str(r#"{"insecure": "true"}"#).unwrap();
        assert_eq!(cfg2.insecure, Some(CertErrorOption(true)));

        let cfg3: TestConfig = serde_json::from_str(r#"{"insecure": "tak"}"#).unwrap();
        assert_eq!(cfg3.insecure, Some(CertErrorOption(true)));

        let cfg4: TestConfig = serde_json::from_str(r#"{"insecure": "false"}"#).unwrap();
        assert_eq!(cfg4.insecure, Some(CertErrorOption(false)));

        let cfg5: TestConfig = serde_json::from_str(r#"{"insecure": 0}"#).unwrap();
        assert_eq!(cfg5.insecure, Some(CertErrorOption(false)));

        let cfg6: TestConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(cfg6.insecure, None);
    }

    #[test]
    fn test_config_defaults_and_aliases() {
        let cfg: HttpRequestConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.method, "GET");
        assert_eq!(cfg.timeout, Some(10));
        assert_eq!(cfg.body_type, "none");

        // the editor stores camelCase keys; hand-written payloads may use
        // the snake_case forms
        let cfg: HttpRequestConfig = serde_json::from_str(
            r#"{"method":"post","url":"http://x","timeoutSeconds":3,
                "bodyType":"json","ignoreCertErrors":"tak",
                "responseVar":"temp","jsonPath":"data.temp"}"#,
        )
        .unwrap();
        assert_eq!(cfg.method, "post");
        assert_eq!(cfg.timeout, Some(3));
        assert_eq!(cfg.body_type, "json");
        assert_eq!(cfg.ignore_cert_errors, Some(CertErrorOption(true)));
        assert_eq!(cfg.response_var.as_deref(), Some("temp"));
        assert_eq!(cfg.json_path.as_deref(), Some("data.temp"));
    }

    // ---- exec integration (local socket server) ---------------------------

    /// One-shot HTTP server on a loopback port: reads the whole request
    /// (head + Content-Length body), answers with `answer`, and hands the
    /// raw request text back through [`Self::request`].
    struct LocalServer {
        addr: std::net::SocketAddr,
        served: Option<std::thread::JoinHandle<String>>,
    }

    impl LocalServer {
        fn serve_once(answer: String) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let served = std::thread::spawn(move || {
                let (mut sock, _) = listener.accept().unwrap();
                use std::io::{Read as _, Write as _};
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                let head_end = loop {
                    let n = sock.read(&mut buf).unwrap();
                    if n == 0 {
                        break raw.len();
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let head = String::from_utf8_lossy(&raw[..head_end]).to_ascii_lowercase();
                let len: usize = head
                    .lines()
                    .find(|l| l.starts_with("content-length:"))
                    .and_then(|l| l.split_once(':').unwrap().1.trim().parse().ok())
                    .unwrap_or(0);
                while raw.len() < head_end + len {
                    let n = sock.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                }
                let _ = sock.write_all(answer.as_bytes());
                String::from_utf8_lossy(&raw).into_owned()
            });
            LocalServer {
                addr,
                served: Some(served),
            }
        }

        fn request(&mut self) -> String {
            self.served.take().unwrap().join().unwrap()
        }
    }

    impl Drop for LocalServer {
        fn drop(&mut self) {
            if let Some(served) = self.served.take() {
                let _ = served.join();
            }
        }
    }

    /// A listener that accepts and then stalls: the tile's timeout must
    /// cut the press off.
    struct StalledServer {
        addr: std::net::SocketAddr,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl StalledServer {
        fn spawn() -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let handle = std::thread::spawn(move || {
                // hold the connection open without ever answering; the
                // client's global timeout is what must end the request
                let (_sock, _) = listener.accept().unwrap();
                std::thread::sleep(std::time::Duration::from_secs(5));
            });
            StalledServer {
                addr,
                handle: Some(handle),
            }
        }
    }

    impl Drop for StalledServer {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn http_button(command: serde_json::Value) -> pulpit_db::ButtonRow {
        pulpit_db::ButtonRow {
            kind: "http-request".into(),
            command: Some(command.to_string()),
            mode: "button".into(),
            ..pulpit_db::ButtonRow::default()
        }
    }

    #[derive(Default)]
    struct VarSink {
        values: Vec<(String, String)>,
    }
    impl pulpit_actions::EventSink for VarSink {
        fn change_board(&mut self, _board_id: i64) {}
        fn app_value(&mut self, key: &str, value: &str) {
            self.values.push((key.into(), value.into()));
        }
    }

    fn exec_http_button(command: serde_json::Value) -> (SqlBackend, VarSink) {
        let backend = crate::SqlBackend::new(
            pulpit_db::Db::open_or_create(std::path::Path::new(":memory:")).unwrap(),
        );
        let mut sink = VarSink::default();
        backend.exec(http_button(command), false, &mut sink);
        (backend, sink)
    }

    #[test]
    fn exec_fires_the_request_and_publishes_the_extracted_value() {
        let body = r#"{"state":"on","raw":1}"#;
        let answer = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let mut server = LocalServer::serve_once(answer);
        let command = serde_json::json!({
            "method": "POST",
            "url": format!("http://{}/api/state", server.addr),
            "headers": "Authorization: Bearer sekret123",
            "bodyType": "json",
            "body": "{\"entity\":\"light.lamp\"}",
            "responseVar": "lamp",
            "jsonPath": "/state"
        });

        let (backend, sink) = exec_http_button(command);

        // the value lands on the sink AND the backend's variable store
        assert_eq!(
            sink.values,
            vec![("lamp".to_string(), "on".to_string())]
        );
        assert_eq!(backend.get_custom_value("lamp").as_deref(), Some("on"));
        assert_eq!(backend.take_last_http_error(), None);

        let request = server.request();
        assert!(request.starts_with("POST /api/state HTTP/1.1"), "{request}");
        assert!(request.contains("{\"entity\":\"light.lamp\"}"), "{request}");
        let headers = request.to_ascii_lowercase();
        assert!(headers.contains("content-type: application/json"), "{request}");
        assert!(headers.contains("authorization: bearer sekret123"), "{request}");
    }

    #[test]
    fn exec_without_a_json_path_publishes_the_status_code() {
        let answer = "HTTP/1.1 201 Created\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        let server = LocalServer::serve_once(answer.into());
        let command = serde_json::json!({
            "url": format!("http://{}/trigger", server.addr),
            "responseVar": "triggered"
        });

        let (_backend, sink) = exec_http_button(command);

        assert_eq!(
            sink.values,
            vec![("triggered".to_string(), "201".to_string())]
        );
    }

    #[test]
    fn exec_unparsable_body_or_missing_path_falls_back_to_the_status() {
        let answer = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n<xml>";
        let server = LocalServer::serve_once(answer.into());
        let command = serde_json::json!({
            "url": format!("http://{}/", server.addr),
            "responseVar": "out",
            "jsonPath": "/nope"
        });

        let (_backend, sink) = exec_http_button(command);

        assert_eq!(sink.values, vec![("out".to_string(), "200".to_string())]);
    }

    #[test]
    fn exec_error_status_is_still_a_completed_request() {
        // http_status_as_error is off for this agent: a 500 answer must
        // publish (the status), not land in the error slot
        let answer = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let server = LocalServer::serve_once(answer.into());
        let command = serde_json::json!({
            "url": format!("http://{}/", server.addr),
            "responseVar": "code"
        });

        let (backend, sink) = exec_http_button(command);

        assert_eq!(sink.values, vec![("code".to_string(), "500".to_string())]);
        assert_eq!(backend.take_last_http_error(), None);
    }

    #[test]
    fn exec_connection_failure_records_the_error() {
        // take a port and release it: nothing is listening there
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = free.local_addr().unwrap();
        drop(free);
        let command = serde_json::json!({ "url": format!("http://{addr}/") });

        let (backend, sink) = exec_http_button(command);

        assert!(sink.values.is_empty());
        let err = backend.take_last_http_error().expect("error recorded");
        assert!(err.contains("HTTP request failed"), "{err}");
        // taken, not peeked
        assert_eq!(backend.take_last_http_error(), None);
    }

    #[test]
    fn exec_timeout_cuts_the_press_off() {
        let stalled = StalledServer::spawn();
        let command = serde_json::json!({
            "url": format!("http://{}/", stalled.addr),
            "timeout": 1
        });

        let started = std::time::Instant::now();
        let (backend, sink) = exec_http_button(command);
        let elapsed = started.elapsed();

        assert!(sink.values.is_empty());
        assert!(backend.take_last_http_error().is_some());
        assert!(
            elapsed < std::time::Duration::from_secs(4),
            "timeout {elapsed:?} must end the press well before the server's 5 s stall"
        );
    }

    #[test]
    fn exec_placeholders_substitute_custom_values() {
        let answer = "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let mut server = LocalServer::serve_once(answer.into());
        let command = serde_json::json!({
            "url": format!("http://{}/relay/{{{{var:relay}}}}", server.addr),
            "headers": "X-Token: {{var:relay}}",
            "responseVar": "echo"
        });

        let backend = crate::SqlBackend::new(
            pulpit_db::Db::open_or_create(std::path::Path::new(":memory:")).unwrap(),
        );
        backend.set_custom_value("relay", "kitchen 1");
        let mut sink = VarSink::default();
        backend.exec(http_button(command), false, &mut sink);

        assert_eq!(
            sink.values,
            vec![("echo".to_string(), "204".to_string())]
        );
        let request = server.request();
        assert!(request.starts_with("GET /relay/kitchen%201 HTTP/1.1"), "{request}");
        let headers = request.to_ascii_lowercase();
        assert!(headers.contains("x-token: kitchen 1"), "{request}");
    }

    #[test]
    fn exec_without_a_url_is_a_claimed_error() {
        let command = serde_json::json!({ "method": "GET" });
        let (backend, _sink) = exec_http_button(command);
        let err = backend.take_last_http_error().expect("error recorded");
        assert!(err.contains("URL"), "{err}");
    }

    #[test]
    fn http_step_inside_a_multiaction_runs_the_native_chain() {
        let answer = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let mut server = LocalServer::serve_once(answer.into());
        let step = serde_json::json!({
            "type": "http-request",
            "command": serde_json::json!({ "url": format!("http://{}/step", server.addr) }).to_string()
        });
        let backend = crate::SqlBackend::new(
            pulpit_db::Db::open_or_create(std::path::Path::new(":memory:")).unwrap(),
        );
        let row = pulpit_db::ButtonRow {
            kind: "multiaction".into(),
            command: Some(serde_json::json!([step]).to_string()),
            mode: "button".into(),
            ..pulpit_db::ButtonRow::default()
        };
        backend.exec(row, false, &mut VarSink::default());

        let request = server.request();
        assert!(request.starts_with("GET /step HTTP/1.1"), "{request}");
    }
}
