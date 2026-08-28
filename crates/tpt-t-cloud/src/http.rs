//! Minimal HTTP/1.1 request parser and response builder (server side).
//!
//! No `hyper`/framework: we parse the small fixed subset the fleet dashboard
//! and MCP endpoint need (methods, path, query, headers, length-delimited
//! body) and emit compact responses. This is driven by the Phase 2 platform
//! event loop in [`crate::server`].

use crate::json::Json;

/// HTTP method. Unknown verbs are kept verbatim under [`Method::Other`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
    /// `DELETE`.
    Delete,
    /// Any other method, preserved as text.
    Other(String),
}

impl Method {
    /// Parses a method token.
    pub fn parse(s: &str) -> Method {
        match s {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "DELETE" => Method::Delete,
            other => Method::Other(other.to_string()),
        }
    }

    /// The canonical token text.
    pub fn as_str(&self) -> &str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Other(s) => s.as_str(),
        }
    }
}

/// Parsing failure classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// Request line or headers malformed.
    Malformed,
}

impl core::fmt::Display for HttpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HttpError::Malformed => f.write_str("malformed HTTP request"),
        }
    }
}

impl std::error::Error for HttpError {}

/// A parsed HTTP request (headers preserved in order).
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// Method.
    pub method: Method,
    /// Path component only (query string stripped into [`Request::query`]).
    pub path: String,
    /// Raw query string (after `?`), empty if absent.
    pub query: String,
    /// Header name/value pairs (names lower-cased).
    pub headers: Vec<(String, String)>,
    /// Body bytes (length-delimited by `Content-Length`).
    pub body: Vec<u8>,
}

impl Request {
    /// Attempts to parse one complete request from `buf`.
    ///
    /// * `Ok(None)` — not enough bytes yet (incomplete headers or body). The
    ///   caller should retain `buf` and call again after more data arrives.
    /// * `Ok(Some((req, total)))` — a complete request spanning `total` bytes
    ///   (so the caller can drain exactly that many from its buffer).
    /// * `Err` — malformed request line/headers.
    pub fn parse(buf: &[u8]) -> Result<Option<(Request, usize)>, HttpError> {
        let hend = match find_subslice(buf, b"\r\n\r\n") {
            Some(p) => p + 4,
            None => return Ok(None),
        };
        let header_text = std::str::from_utf8(&buf[..hend.saturating_sub(4)])
            .map_err(|_| HttpError::Malformed)?;
        let mut lines = header_text.split("\r\n");
        let start = lines.next().ok_or(HttpError::Malformed)?;
        let parts: Vec<&str> = start.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(HttpError::Malformed);
        }
        let method = Method::parse(parts[0]);
        let (path, query) = split_query(parts[1]);

        let mut headers = Vec::new();
        let mut content_length = 0usize;
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                let k = k.trim().to_lowercase();
                let v = v.trim().to_string();
                if k == "content-length" {
                    content_length = v.parse().unwrap_or(0);
                }
                headers.push((k, v));
            }
        }

        let total = hend + content_length;
        if buf.len() < total {
            return Ok(None);
        }
        let body = buf[hend..total].to_vec();
        Ok(Some((
            Request {
                method,
                path,
                query,
                headers,
                body,
            },
            total,
        )))
    }
}

fn split_query(target: &str) -> (String, String) {
    match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.to_string(), String::new()),
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// An HTTP response to be serialized to the wire.
#[derive(Debug, Clone)]
pub struct Response {
    /// Status code.
    pub status: u16,
    /// Header pairs (names preserved as given).
    pub headers: Vec<(String, String)>,
    /// Body bytes.
    pub body: Vec<u8>,
}

impl Response {
    /// Builds a JSON response.
    pub fn json(code: u16, body: Json) -> Self {
        Self::with_body(code, "application/json", body.to_string().into_bytes())
    }

    /// Builds a plain-text response.
    pub fn text(code: u16, body: &str) -> Self {
        Self::with_body(code, "text/plain; charset=utf-8", body.as_bytes().to_vec())
    }

    /// Builds an HTML response.
    pub fn html(code: u16, body: &str) -> Self {
        Self::with_body(code, "text/html; charset=utf-8", body.as_bytes().to_vec())
    }

    /// Builds a response carrying a body with the given content type.
    pub fn with_body(code: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status: code,
            headers: vec![
                ("content-type".to_string(), content_type.to_string()),
                ("content-length".to_string(), body.len().to_string()),
            ],
            body,
        }
    }

    /// A plain-text error response.
    pub fn error(code: u16, msg: &str) -> Self {
        Self::text(code, msg)
    }

    /// Serializes to wire bytes (HTTP/1.1, `Connection: close`).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.body.len());
        out.extend_from_slice(
            format!("HTTP/1.1 {} {}\r\n", self.status, status_text(self.status)).as_bytes(),
        );
        for (k, v) in &self.headers {
            out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
        }
        out.extend_from_slice(b"connection: close\r\n\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

/// Returns the canonical reason phrase for a status code (subset used here).
pub fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        422 => "Unprocessable Entity",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_get_no_body() {
        let raw = b"GET /api/health HTTP/1.1\r\nHost: x\r\n\r\n";
        let (req, total) = Request::parse(raw).unwrap().unwrap();
        assert_eq!(req.method, Method::Get);
        assert_eq!(req.path, "/api/health");
        assert_eq!(req.query, "");
        assert_eq!(total, raw.len());
        assert!(req.body.is_empty());
    }

    #[test]
    fn parse_post_with_query_and_body() {
        let raw = b"POST /mcp?trace=1 HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        let (req, total) = Request::parse(raw).unwrap().unwrap();
        assert_eq!(req.method, Method::Post);
        assert_eq!(req.path, "/mcp");
        assert_eq!(req.query, "trace=1");
        assert_eq!(req.body, b"hello");
        assert_eq!(total, raw.len());
    }

    #[test]
    fn parse_incomplete_returns_none() {
        let raw = b"POST /mcp HTTP/1.1\r\nContent-Length: 50\r\n\r\n short";
        assert!(Request::parse(raw).unwrap().is_none());
        // No double CRLF yet either.
        assert!(
            Request::parse(b"GET /x HTTP/1.1\r\nHost:")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn response_serializes_with_headers() {
        let r = Response::json(200, Json::obj_str(&[("status", "ok")]));
        let bytes = r.to_bytes();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(s.contains("content-type: application/json"));
        assert!(s.contains("connection: close"));
        assert!(s.trim_end().ends_with("{\"status\":\"ok\"}"));
    }
}
