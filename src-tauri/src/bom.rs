//! JSON BoM file load, the job-card API POST, and MSSQL inspect.
//! `inspect_bom` reads the saved source and runs the SQL file when that source is MSSQL.
//! These commands do not open a camera.

use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use url::Url;

use crate::local_config;

const SAMPLE_BOM: &str = "sample-bom.json";
const API_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const TIMEOUT_MESSAGE: &str = "The BoM API did not respond in time.";

#[tauri::command]
pub fn load_bom_json() -> Result<Value, String> {
    read_sample_bom(&local_config::project_root()?)
}

#[tauri::command]
pub fn inspect_bom(barcode: String) -> Result<Value, String> {
    let root = local_config::project_root()?;
    let config = local_config::load_document(&root)?;
    Ok(crate::bom_sql::inspect(&barcode, &config))
}

#[tauri::command]
pub fn post_bom_api(job_card_number: String) -> Result<Value, String> {
    let root = local_config::project_root()?;
    let config = local_config::load_document(&root)?;
    post_job_card(&config, &job_card_number, API_TIMEOUT)
}

fn sample_bom_path(root: &Path) -> std::path::PathBuf {
    root.join("vision").join(SAMPLE_BOM)
}

fn read_sample_bom(root: &Path) -> Result<Value, String> {
    let path = sample_bom_path(root);
    match fs::read(&path) {
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|_| "vision/sample-bom.json is not valid JSON.".to_string())?;
            if !value.is_object() {
                return Err("vision/sample-bom.json must be a JSON object.".to_string());
            }
            Ok(value)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(err) => Err(format!("Could not read vision/sample-bom.json: {err}")),
    }
}

fn job_card_number(value: &str) -> Result<i64, String> {
    let text = value.trim();
    if text.is_empty() || text.len() > 18 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Enter a job card number.".to_string());
    }
    text.parse()
        .map_err(|_| "Enter a job card number.".to_string())
}

fn api_text(config: &Value, name: &str) -> String {
    config
        .get("api")
        .and_then(|value| value.as_object())
        .and_then(|api| api.get(name))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn join_api_url(base_url: &str, path: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let suffix = path.trim();
    if suffix.is_empty() {
        return base.to_string();
    }
    if suffix.starts_with('/') {
        format!("{base}{suffix}")
    } else {
        format!("{base}/{suffix}")
    }
}

fn scheme_error() -> String {
    "API base URL must start with http:// or https://.".to_string()
}

fn validated_endpoint(base_url: &str, path: &str) -> Result<Url, String> {
    if base_url.is_empty() {
        return Err("API base URL is not set. Add it in Settings.".to_string());
    }
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(scheme_error());
    }
    let base = Url::parse(base_url).map_err(|_| scheme_error())?;
    if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
        return Err(scheme_error());
    }
    let joined = join_api_url(base_url, path);
    let url = Url::parse(&joined).map_err(|_| scheme_error())?;
    if url.scheme() != base.scheme()
        || url.host_str() != base.host_str()
        || url.port_or_known_default() != base.port_or_known_default()
    {
        return Err(scheme_error());
    }
    Ok(url)
}

fn header_safe(value: &str) -> bool {
    !value
        .bytes()
        .any(|byte| byte == b'\r' || byte == b'\n' || byte == 0)
}

fn scrub(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "********")
    }
}

fn clip(text: &str) -> String {
    text.chars().take(200).collect()
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        _ => String::new(),
    }
}

fn payload_message(payload: &Value) -> String {
    let Some(object) = payload.as_object() else {
        return String::new();
    };
    for name in ["message", "error", "title", "detail", "status"] {
        let Some((_, value)) = object
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
        else {
            continue;
        };
        let text = scalar_text(value);
        if text.is_empty() {
            continue;
        }
        if name == "status"
            && matches!(
                text.to_ascii_lowercase().as_str(),
                "ok" | "success" | "pass" | "true" | "200"
            )
        {
            continue;
        }
        return text;
    }
    String::new()
}

fn error_detail(body: &[u8], key: &str) -> String {
    if let Ok(payload) = serde_json::from_slice::<Value>(body) {
        let detail = payload_message(&payload);
        if detail.is_empty() {
            return String::new();
        }
        return clip(&scrub(&detail, key));
    }
    let text = String::from_utf8_lossy(body);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    clip(&scrub(trimmed, key))
}

fn interpret(status: u16, body: &[u8], key: &str) -> Result<Value, String> {
    if status != 200 {
        let detail = error_detail(body, key);
        if detail.is_empty() {
            return Err(format!("BoM API returned HTTP {status}."));
        }
        return Err(format!("BoM API returned HTTP {status}. {detail}"));
    }
    let payload: Value =
        serde_json::from_slice(body).map_err(|_| "BoM API returned invalid JSON.".to_string())?;
    match payload.get("data") {
        Some(data) if data.is_object() => Ok(data.clone()),
        _ => {
            let detail = clip(&scrub(&payload_message(&payload), key));
            if detail.is_empty() {
                Err("BoM API response has no data object.".to_string())
            } else {
                Err(detail)
            }
        }
    }
}

fn is_timeout(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

fn map_io(err: std::io::Error, key: &str) -> String {
    if is_timeout(&err) {
        return TIMEOUT_MESSAGE.to_string();
    }
    if err.kind() == std::io::ErrorKind::UnexpectedEof {
        return "Could not reach the BoM API. The connection closed before a response.".to_string();
    }
    let text = scrub(&err.to_string(), key);
    let text = if text.trim().is_empty() {
        "connection failed".to_string()
    } else {
        text
    };
    format!("Could not reach the BoM API. {text}")
}

fn map_tls(err: &native_tls::Error, key: &str) -> String {
    let mut source = err.source();
    while let Some(item) = source {
        if let Some(io_err) = item.downcast_ref::<std::io::Error>() {
            if is_timeout(io_err) {
                return TIMEOUT_MESSAGE.to_string();
            }
        }
        source = item.source();
    }
    let text = scrub(&err.to_string(), key);
    let text = if text.trim().is_empty() {
        "connection failed".to_string()
    } else {
        text
    };
    format!("Could not reach the BoM API. {text}")
}

enum ReadFail {
    Io(std::io::Error),
    Closed,
    Invalid,
    TooLarge,
}

fn map_read(err: ReadFail, key: &str) -> String {
    match err {
        ReadFail::Io(err) => map_io(err, key),
        ReadFail::Closed => {
            "Could not reach the BoM API. The connection closed before a response.".to_string()
        }
        ReadFail::Invalid => {
            "Could not reach the BoM API. The response was not valid HTTP.".to_string()
        }
        ReadFail::TooLarge => "BoM API response is too large.".to_string(),
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .rev()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn parse_head(head: &[u8]) -> Option<(u16, Vec<(String, String)>)> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next()?;
    let mut parts = status_line.split(' ');
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    let code = parts.next()?.parse().ok()?;
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':')?;
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    Some((code, headers))
}

fn is_chunked(headers: &[(String, String)]) -> bool {
    header_value(headers, "transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
}

fn read_crlf_line<R: Read>(input: &mut R, max: usize) -> Result<String, ReadFail> {
    let mut buf = Vec::new();
    loop {
        if buf.len() > max {
            return Err(ReadFail::TooLarge);
        }
        let mut byte = [0u8; 1];
        match input.read(&mut byte).map_err(ReadFail::Io)? {
            0 => return Err(ReadFail::Closed),
            _ => buf.push(byte[0]),
        }
        if buf.ends_with(b"\r\n") {
            buf.truncate(buf.len() - 2);
            return Ok(String::from_utf8_lossy(&buf).into_owned());
        }
    }
}

fn read_chunked<R: Read>(input: &mut R) -> Result<Vec<u8>, ReadFail> {
    let mut body = Vec::new();
    loop {
        let line = read_crlf_line(input, 1024)?;
        let size_text = line.split(';').next().unwrap_or("").trim();
        if size_text.is_empty() {
            return Err(ReadFail::Invalid);
        }
        let size = usize::from_str_radix(size_text, 16).map_err(|_| ReadFail::Invalid)?;
        if size == 0 {
            loop {
                let trailer = read_crlf_line(input, MAX_HEADER_BYTES)?;
                if trailer.is_empty() {
                    break;
                }
            }
            break;
        }
        if body.len().saturating_add(size) > MAX_BODY_BYTES {
            return Err(ReadFail::TooLarge);
        }
        let mut chunk = vec![0u8; size];
        input.read_exact(&mut chunk).map_err(ReadFail::Io)?;
        body.extend_from_slice(&chunk);
        let mut crlf = [0u8; 2];
        input.read_exact(&mut crlf).map_err(ReadFail::Io)?;
        if &crlf != b"\r\n" {
            return Err(ReadFail::Invalid);
        }
    }
    Ok(body)
}

fn read_body<R: Read>(
    reader: &mut R,
    headers: &[(String, String)],
    prefetch: Vec<u8>,
) -> Result<Vec<u8>, ReadFail> {
    let mut input = std::io::Cursor::new(prefetch).chain(reader);
    if is_chunked(headers) {
        return read_chunked(&mut input);
    }
    if let Some(raw) = header_value(headers, "content-length") {
        let len: usize = raw.trim().parse().map_err(|_| ReadFail::Invalid)?;
        if len > MAX_BODY_BYTES {
            return Err(ReadFail::TooLarge);
        }
        let mut buf = vec![0u8; len];
        input.read_exact(&mut buf).map_err(ReadFail::Io)?;
        return Ok(buf);
    }
    let mut buf = Vec::new();
    input
        .take((MAX_BODY_BYTES as u64) + 1)
        .read_to_end(&mut buf)
        .map_err(ReadFail::Io)?;
    if buf.len() > MAX_BODY_BYTES {
        return Err(ReadFail::TooLarge);
    }
    Ok(buf)
}

struct RawHttp {
    status: u16,
    body: Vec<u8>,
}

fn read_response<R: Read>(reader: &mut R) -> Result<RawHttp, ReadFail> {
    let mut pending = Vec::new();
    let mut tmp = [0u8; 4096];
    let split = loop {
        if let Some(index) = find_header_end(&pending) {
            break index;
        }
        if pending.len() > MAX_HEADER_BYTES {
            return Err(ReadFail::TooLarge);
        }
        let read = reader.read(&mut tmp).map_err(ReadFail::Io)?;
        if read == 0 {
            return Err(ReadFail::Closed);
        }
        pending.extend_from_slice(&tmp[..read]);
    };
    if split > MAX_HEADER_BYTES {
        return Err(ReadFail::TooLarge);
    }
    let head = pending[..split].to_vec();
    let prefetch = pending[split + 4..].to_vec();
    let (status, headers) = parse_head(&head).ok_or(ReadFail::Invalid)?;
    let body = read_body(reader, &headers, prefetch)?;
    Ok(RawHttp { status, body })
}

fn origin_form(url: &Url) -> String {
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    }
}

fn host_header(url: &Url) -> String {
    let host = url.host_str().unwrap_or("");
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    match (url.scheme(), url.port()) {
        ("http", Some(port)) if port != 80 => format!("{host}:{port}"),
        ("https", Some(port)) if port != 443 => format!("{host}:{port}"),
        _ => host,
    }
}

fn build_request(url: &Url, key: &str, body: &[u8]) -> Vec<u8> {
    let head = format!(
        "POST {target} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json\r\n\
         X-Api-Key: {key}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n",
        target = origin_form(url),
        host = host_header(url),
        len = body.len(),
    );
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn connect_tcp(url: &Url, timeout: Duration) -> Result<TcpStream, std::io::Error> {
    let host = url
        .host_str()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing host"))?;
    let port = url.port_or_known_default().unwrap_or(80);
    let mut last = None;
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(err) => last = Some(err),
        }
    }
    Err(last
        .unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "connection failed")))
}

fn write_and_read<S: Read + Write>(
    stream: &mut S,
    url: &Url,
    key: &str,
    body: &[u8],
) -> Result<RawHttp, String> {
    stream
        .write_all(&build_request(url, key, body))
        .map_err(|err| map_io(err, key))?;
    stream.flush().map_err(|err| map_io(err, key))?;
    read_response(stream).map_err(|err| map_read(err, key))
}

fn exchange(url: &Url, key: &str, body: &[u8], timeout: Duration) -> Result<RawHttp, String> {
    let tcp = connect_tcp(url, timeout).map_err(|err| map_io(err, key))?;
    tcp.set_read_timeout(Some(timeout))
        .map_err(|err| map_io(err, key))?;
    tcp.set_write_timeout(Some(timeout))
        .map_err(|err| map_io(err, key))?;
    let _ = tcp.set_nodelay(true);
    if url.scheme() == "https" {
        let connector = native_tls::TlsConnector::builder()
            .build()
            .map_err(|err| map_tls(&err, key))?;
        let host = url.host_str().unwrap_or("");
        let mut tls = connector.connect(host, tcp).map_err(|err| match err {
            native_tls::HandshakeError::WouldBlock(_) => TIMEOUT_MESSAGE.to_string(),
            native_tls::HandshakeError::Failure(err) => map_tls(&err, key),
        })?;
        write_and_read(&mut tls, url, key, body)
    } else {
        let mut tcp = tcp;
        write_and_read(&mut tcp, url, key, body)
    }
}

fn post_job_card(config: &Value, job_card: &str, timeout: Duration) -> Result<Value, String> {
    let number = job_card_number(job_card)?;
    let key = api_text(config, "key");
    if key.is_empty() {
        return Err("API key is not set. Add it in Settings.".to_string());
    }
    if !header_safe(&key) {
        return Err("Could not reach the BoM API. The API key is not a single line.".to_string());
    }
    let base_url = api_text(config, "baseUrl");
    let path = api_text(config, "path");
    let url = validated_endpoint(&base_url, &path)?;
    let body = serde_json::to_vec(&serde_json::json!({ "jobCardNumber": number }))
        .map_err(|_| "BoM API lookup failed.".to_string())?;
    let response = exchange(&url, &key, &body, timeout)?;
    // A 3xx is returned as that status. The key is not sent to another host.
    interpret(response.status, &response.body, &key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    fn config(base: &str, path: &str, key: &str) -> Value {
        serde_json::json!({
            "api": { "baseUrl": base, "path": path, "key": key }
        })
    }

    fn http_response(status: u16, reason: &str, extra_headers: &str, body: &str) -> Vec<u8> {
        let head = format!(
            "HTTP/1.1 {status} {reason}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {len}\r\n\
             Connection: close\r\n\
             {extra_headers}\
             \r\n\
             {body}",
            len = body.len(),
        );
        head.into_bytes()
    }

    fn request_complete(buf: &[u8]) -> bool {
        let Some(split) = find_header_end(buf) else {
            return false;
        };
        let head = &buf[..split];
        let Ok(text) = std::str::from_utf8(head) else {
            return true;
        };
        let length = text.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        });
        match length {
            Some(length) => buf.len() >= split + 4 + length,
            None => true,
        }
    }

    fn spawn_server(response: Vec<u8>) -> (u16, mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
            let mut received = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match sock.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        received.extend_from_slice(&buf[..n]);
                        if request_complete(&received) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = sock.write_all(&response);
            let _ = sock.flush();
            let _ = tx.send(received);
        });
        (port, rx)
    }

    fn contains_bytes(buf: &[u8], needle: &[u8]) -> bool {
        buf.windows(needle.len()).any(|window| window == needle)
    }

    #[test]
    fn job_card_numbers_are_digits_up_to_18() {
        assert_eq!(job_card_number("17834402").unwrap(), 17_834_402);
        assert_eq!(job_card_number(" 007 ").unwrap(), 7);
        assert_eq!(
            job_card_number(&"9".repeat(18)).unwrap(),
            999_999_999_999_999_999
        );
        assert_eq!(job_card_number("").unwrap_err(), "Enter a job card number.");
        assert_eq!(
            job_card_number("12a").unwrap_err(),
            "Enter a job card number."
        );
        assert_eq!(
            job_card_number(&"1".repeat(19)).unwrap_err(),
            "Enter a job card number."
        );
    }

    #[test]
    fn api_url_joins_the_base_and_the_path() {
        assert_eq!(
            join_api_url("http://example.com/", "jobs"),
            "http://example.com/jobs"
        );
        assert_eq!(
            join_api_url("http://example.com", "/jobs"),
            "http://example.com/jobs"
        );
        assert_eq!(
            join_api_url("http://example.com/v1/", "jobs"),
            "http://example.com/v1/jobs"
        );
        assert_eq!(
            join_api_url("https://example.com", ""),
            "https://example.com"
        );
        let url = validated_endpoint("https://example.com/v1", "jobs").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.path(), "/v1/jobs");
        assert_eq!(
            validated_endpoint("", "/jobs").unwrap_err(),
            "API base URL is not set. Add it in Settings."
        );
        assert_eq!(
            validated_endpoint("ftp://example.com", "/jobs").unwrap_err(),
            "API base URL must start with http:// or https://."
        );
    }

    #[test]
    fn api_settings_are_checked_before_a_connection() {
        let missing_key = post_job_card(&config("", "", ""), "12", API_TIMEOUT).unwrap_err();
        assert_eq!(missing_key, "API key is not set. Add it in Settings.");
        let missing_base =
            post_job_card(&config("", "/jobs", "secret"), "12", API_TIMEOUT).unwrap_err();
        assert_eq!(missing_base, "API base URL is not set. Add it in Settings.");
        let bad_scheme = post_job_card(
            &config("ftp://example.com", "/jobs", "secret"),
            "12",
            API_TIMEOUT,
        )
        .unwrap_err();
        assert_eq!(
            bad_scheme,
            "API base URL must start with http:// or https://."
        );
        let bad_number = post_job_card(
            &config("http://example.com", "/jobs", "secret"),
            "nope",
            API_TIMEOUT,
        )
        .unwrap_err();
        assert_eq!(bad_number, "Enter a job card number.");
        let injected = post_job_card(
            &config("http://127.0.0.1:9", "/", "a\r\nHost: evil"),
            "1",
            API_TIMEOUT,
        )
        .unwrap_err();
        assert_eq!(
            injected,
            "Could not reach the BoM API. The API key is not a single line."
        );
    }

    #[test]
    fn response_messages_match_the_sidecar() {
        let data = interpret(
            200,
            br#"{"status":"pass","data":{"kitIC":"6PBBE039","parts":[]}}"#,
            "secret",
        )
        .unwrap();
        assert_eq!(data["kitIC"], "6PBBE039");
        assert!(data.get("status").is_none());

        assert_eq!(
            interpret(404, br#"{"message":"missing","status":"ok"}"#, "secret").unwrap_err(),
            "BoM API returned HTTP 404. missing"
        );
        assert_eq!(
            interpret(500, b"oops secret-key", "secret-key").unwrap_err(),
            "BoM API returned HTTP 500. oops ********"
        );
        assert_eq!(
            interpret(200, br#"{"message":"denied secret-key"}"#, "secret-key").unwrap_err(),
            "denied ********"
        );
        assert_eq!(
            interpret(200, br#"{"status":"ok"}"#, "secret").unwrap_err(),
            "BoM API response has no data object."
        );
        assert_eq!(
            interpret(200, br#"{"status":"No row"}"#, "secret").unwrap_err(),
            "No row"
        );
        assert_eq!(
            interpret(200, b"not-json", "secret").unwrap_err(),
            "BoM API returned invalid JSON."
        );
        assert_eq!(
            interpret(302, b"", "secret").unwrap_err(),
            "BoM API returned HTTP 302."
        );
    }

    #[test]
    fn post_sends_the_exact_api_key_header_and_returns_data() {
        let body = r#"{"data":{"kitIC":"6PBBE039","des":"Kit","parts":[]}}"#;
        let (port, received) = spawn_server(http_response(200, "OK", "", body));
        let value = post_job_card(
            &config(&format!("http://127.0.0.1:{port}"), "/bom", "test-key"),
            "00017834402",
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(value["kitIC"], "6PBBE039");
        let bytes = received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(contains_bytes(&bytes, b"X-Api-Key: test-key\r\n"));
        assert!(!contains_bytes(&bytes, b"X-api-key:"));
        assert!(!contains_bytes(&bytes, b"x-api-key:"));
        assert!(contains_bytes(&bytes, b"POST /bom HTTP/1.1\r\n"));
        assert!(contains_bytes(&bytes, br#"{"jobCardNumber":17834402}"#));
    }

    #[test]
    fn post_reads_a_chunked_data_object() {
        let payload = br#"{"data":{"kitIC":"A"}}"#;
        let raw = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{size:x}\r\n{}\r\n0\r\n\r\n",
            std::str::from_utf8(payload).unwrap(),
            size = payload.len(),
        );
        let (port, _) = spawn_server(raw.into_bytes());
        let value = post_job_card(
            &config(&format!("http://127.0.0.1:{port}"), "", "test-key"),
            "7",
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(value["kitIC"], "A");
    }

    #[test]
    fn a_silent_server_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buf = [0u8; 1024];
                loop {
                    match sock.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            }
        });
        let err = post_job_card(
            &config(&format!("http://127.0.0.1:{port}"), "/bom", "test-key"),
            "12",
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert_eq!(err, TIMEOUT_MESSAGE);
    }

    #[test]
    fn reach_errors_scrub_the_api_key() {
        let err = map_io(
            std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "connection failed test-key",
            ),
            "test-key",
        );
        assert_eq!(
            err,
            "Could not reach the BoM API. connection failed ********"
        );
    }

    #[test]
    fn a_socket_that_closes_is_a_reach_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            if let Ok((sock, _)) = listener.accept() {
                drop(sock);
            }
        });
        let err = post_job_card(
            &config(&format!("http://127.0.0.1:{port}"), "/bom", "test-key"),
            "12",
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert!(err.starts_with("Could not reach the BoM API."), "{err}");
        assert!(!err.contains("test-key"));
    }

    #[test]
    fn sample_bom_missing_file_is_an_empty_object() {
        let root = std::env::temp_dir().join(format!(
            "ls-vision-bom-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("vision")).unwrap();
        let value = read_sample_bom(&root).unwrap();
        assert_eq!(value, serde_json::json!({}));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sample_bom_reads_an_object_and_rejects_other_json() {
        let root = std::env::temp_dir().join(format!(
            "ls-vision-bom-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("vision")).unwrap();
        let path = root.join("vision").join("sample-bom.json");
        fs::write(&path, br#"{"6PBBK001":{"des":"Kit","parts":[]}}"#).unwrap();
        let value = read_sample_bom(&root).unwrap();
        assert_eq!(value["6PBBK001"]["des"], "Kit");

        fs::write(&path, b"[").unwrap();
        assert_eq!(
            read_sample_bom(&root).unwrap_err(),
            "vision/sample-bom.json is not valid JSON."
        );
        fs::write(&path, b"[]").unwrap();
        assert_eq!(
            read_sample_bom(&root).unwrap_err(),
            "vision/sample-bom.json must be a JSON object."
        );
        let _ = fs::remove_dir_all(&root);
    }
}
