//! The HTTP side of the node (`listenNew` and friends in `Transient.Move.Internals`): a node listens for other nodes and for
//! HTTP clients on the same port. The first line tells them apart: `CLOS ...` is a node; anything else is a request.
//!
//! A request to `/<closure>/<params>` is a message to the continuation point (endpoint) with that name; the params are
//! the rest of the log that the continuation reads (`logged` reads the values from the path). The response is chunked
//! JSON that stays open: each `minput`/`moutput` reached by the continuation adds a chunk.

use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

/// The request line and the headers of the HTTP request being served (`HTTPHeaders`)
#[derive(Clone, Debug, Default)]
pub struct HttpHeaders {
    pub method: String,
    pub uri: String,
    pub version: String,
    pub headers: Vec<(String, String)>,
}

impl HttpHeaders {
    /// A header by name, ignoring case
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// A request: its headers and the body
#[derive(Clone)]
pub struct HttpRequest {
    pub head: HttpHeaders,
    pub body: Vec<u8>,
}

/// Reads one request. `first_line` is the request line if it was already read (to tell nodes from clients).
pub(crate) fn read_request(reader: &mut BufReader<TcpStream>, first_line: Option<String>) -> Option<HttpRequest> {
    let line = match first_line {
        Some(l) => l,
        None => {
            let mut l = String::new();
            if reader.read_line(&mut l).ok()? == 0 {
                return None;
            }
            l
        }
    };
    let mut parts = line.trim_end().splitn(3, ' ');
    let (method, uri, version) = (parts.next()?.to_string(), parts.next()?.to_string(), parts.next().unwrap_or("HTTP/1.1").to_string());
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        reader.read_line(&mut l).ok()?;
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        let (k, v) = l.split_once(':')?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let head = HttpHeaders { method, uri, version, headers };
    let mut body = Vec::new();
    if head.method == "POST" {
        if let Some(len) = head.get("Content-Length").and_then(|l| l.parse::<usize>().ok()) {
            body = vec![0; len];
            reader.read_exact(&mut body).ok()?;
        }
    }
    Some(HttpRequest { head, body })
}

/// Decodes `%XX` escapes (`unEscapeString`)
pub fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The value of a cookie in the `Cookie` header (`getCookie`)
pub fn cookie_of(head: &HttpHeaders, name: &str) -> Option<String> {
    let all = head.get("Cookie")?;
    all.split(';').filter_map(|c| c.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
}

/// The addressing of a request: the session and closure of the receiver, the session and closure to respond to, and what
/// remains of the path, which is the rest of the log. There are several formats (`listenNew`):
///
/// * `/<closure>/<session>/<this closure>/<session>/...`: everything is given
/// * `/<closure>/T<session>/...`: the session of the receiver
/// * `/<closure>/S<session>/...`: the endpoint of another session; with the cookie of the caller it is a jump (`#`)
/// * `/<closure>/...` with the `state` cookie: the session of the cookie
/// * `/<closure>/...` without a cookie: session 0
///
/// A parameter that starts with `T` or `S` is taken as a session, as in the original.
pub(crate) fn route(path: &str, cookie: Option<&str>) -> (String, String, String, String, String) {
    // the closure is up to the first '/'
    let (closure, params) = match path.split_once('/') {
        Some((c, p)) => (c.to_string(), p),
        None => (path.to_string(), ""),
    };
    // everything given: <session>/<closure>/<session>/
    let seg: Vec<&str> = params.splitn(4, '/').collect();
    if seg.len() == 4 {
        return (closure, seg[0].into(), seg[1].into(), seg[2].into(), seg[3].into());
    }
    if let Some(rest) = params.strip_prefix('T') {
        let (s1, tail) = rest.split_once('/').map_or((rest, ""), |(a, b)| (a, b));
        let rest_of = if rest.contains('/') { format!("/{tail}") } else { String::new() };
        return (closure, s1.into(), "0".into(), "0".into(), rest_of);
    }
    if let Some(rest) = params.strip_prefix('S') {
        let (s1, tail) = rest.split_once('/').unwrap_or((rest, ""));
        let s1 = format!("#{s1}-{}", cookie.unwrap_or("0"));
        return (closure, s1, "0".into(), "0".into(), tail.into());
    }
    if let Some(c) = cookie {
        return (closure, c.into(), "0".into(), "0".into(), params.into());
    }
    (closure, "0".into(), "0".into(), "0".into(), params.into())
}

/// The type of a file by its extension (`defaultMimeLookup`, the usual ones)
fn mime_of(file: &str) -> &'static str {
    match file.rsplit('.').next().unwrap_or("") {
        "html" | "htm" => "text/html",
        "js" | "mjs" => "application/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "txt" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Serves a file of the directory `./static` of the node (`servePage`): `/` and `/index.html` are `index.html`, and
/// `/file/<name>` is `<name>`. The response is HTTP/1.0 and closes the connection. A path with `..` is not served (the
/// original only dropped the dots at the start of the path).
///
/// The original also sets a `state` cookie when the request has none, so that the browser has a session from the page.
/// Here it does not: the session is created by the first endpoint that the page calls (`init_session`), and a cookie
/// that the node did not create names a session whose endpoints do not exist, so the first call would fail.
pub(crate) fn serve_page(stream: &HttpSock, path: &str) {
    let file = match path.split_once('/') {
        Some(("file", rest)) if !rest.is_empty() => rest,
        _ if path.is_empty() => "index.html",
        _ => path,
    };
    let file = file.trim_start_matches('.');
    let content = (!file.split('/').any(|c| c == "..")).then(|| std::fs::read(format!("./static/{file}")).ok()).flatten();
    let Some(content) = content else {
        stream.write(b"HTTP/1.0 404 Not Found\r\nContent-Length: 13\r\nConnection: close\r\n\r\nNot Found 404");
        return;
    };
    let response = format!(
        "HTTP/1.0 200 OK\r\nContent-Type: {}\r\nConnection: close\r\nContent-Length: {}\r\nSet-Cookie:cookie={}\r\n\r\n",
        mime_of(file),
        content.len(),
        crate::net::cluster_cookie_now()
    );
    stream.write(response.as_bytes());
    stream.write(&content);
    stream.shutdown();
}

/// Writes a chunk of a chunked response (`toHex l <> "\r\n" <> tosend <> "\r\n"`)
pub(crate) fn chunk(body: &str) -> String {
    format!("{:X}\r\n{body}\r\n", body.len())
}

/// What is sent when a request is routed to its continuation point: the response is chunked JSON. The headers are left
/// open, without the blank line that ends them, so that the cookies that the program sets go inside them (`HttpSock`).
pub(crate) const RESPONSE_HEAD: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n";

/// The socket of an HTTP client, and whether the headers of the response are still open
pub struct HttpSock {
    stream: Mutex<TcpStream>,
    head_open: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for HttpSock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HttpSock")
    }
}

impl HttpSock {
    pub(crate) fn new(stream: TcpStream) -> HttpSock {
        HttpSock { stream: Mutex::new(stream), head_open: false.into() }
    }

    /// Writes bytes as they are
    pub(crate) fn write(&self, bytes: &[u8]) {
        let _ = self.stream.lock().unwrap().write_all(bytes);
    }

    /// Sends the start of a response and leaves its headers open
    pub(crate) fn start_response(&self) {
        self.write(RESPONSE_HEAD.as_bytes());
        self.head_open.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.stream.lock().unwrap().shutdown(std::net::Shutdown::Both);
    }

    /// Ends the headers of the response (the blank line) if they are open
    pub fn close_headers(&self) {
        if self.head_open.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.write(b"\r\n");
        }
    }

    /// Sends data of the response: the first thing that is not a header ends the headers (`msend` of an `HTTP2Node`)
    pub(crate) fn send(&self, bytes: &str) {
        if !bytes.starts_with("Set-Cookie:") {
            self.close_headers();
        }
        self.write(bytes.as_bytes());
    }
}
