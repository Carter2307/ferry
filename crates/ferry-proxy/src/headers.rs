//! Header rewriting between client and upstream: hop-by-hop stripping,
//! upgrade detection and `X-Forwarded-*`.

use std::net::IpAddr;

use http::header::{
    CONNECTION, COOKIE, EXPECT, HOST, HeaderMap, HeaderName, HeaderValue, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE,
    TRAILER, TRANSFER_ENCODING, UPGRADE,
};

pub(crate) const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
pub(crate) const X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");
pub(crate) const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");
pub(crate) const X_FORWARDED_PORT: HeaderName = HeaderName::from_static("x-forwarded-port");
pub(crate) const X_REAL_IP: HeaderName = HeaderName::from_static("x-real-ip");
pub(crate) const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

const KEEP_ALIVE: HeaderName = HeaderName::from_static("keep-alive");
const PROXY_CONNECTION: HeaderName = HeaderName::from_static("proxy-connection");

/// Headers that only apply to a single connection (RFC 9110 §7.6.1) plus the
/// non-standard ones proxies conventionally drop.
const HOP_BY_HOP: [HeaderName; 9] = [
    CONNECTION,
    KEEP_ALIVE,
    PROXY_CONNECTION,
    PROXY_AUTHENTICATE,
    PROXY_AUTHORIZATION,
    TE,
    TRAILER,
    TRANSFER_ENCODING,
    UPGRADE,
];

/// Comma-separated tokens of every `Connection` header value.
fn connection_tokens(headers: &HeaderMap) -> impl Iterator<Item = &str> {
    headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// Remove hop-by-hop headers, including every header named by `Connection`.
pub(crate) fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let listed: Vec<HeaderName> =
        connection_tokens(headers).filter_map(|t| HeaderName::from_bytes(t.as_bytes()).ok()).collect();
    for name in listed {
        headers.remove(name);
    }
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
}

/// `Upgrade` values to forward if this HTTP/1.x request asks for a protocol
/// upgrade we can tunnel (e.g. `websocket`), `None` otherwise. `h2c` upgrades
/// are not tunnelled: the request is proxied as plain HTTP/1.1.
pub(crate) fn requested_upgrade(version: http::Version, headers: &HeaderMap) -> Option<Vec<HeaderValue>> {
    if version != http::Version::HTTP_11 && version != http::Version::HTTP_10 {
        return None;
    }
    if !connection_tokens(headers).any(|t| t.eq_ignore_ascii_case("upgrade")) {
        return None;
    }
    let values: Vec<HeaderValue> = headers.get_all(UPGRADE).iter().cloned().collect();
    let only_h2c = values
        .iter()
        .flat_map(|v| v.to_str().unwrap_or("").split(','))
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .all(|p| p.eq_ignore_ascii_case("h2c"));
    if values.is_empty() || only_h2c { None } else { Some(values) }
}

/// Re-add `Connection: upgrade` + `Upgrade` after hop-by-hop stripping.
pub(crate) fn restore_upgrade(headers: &mut HeaderMap, upgrade: Vec<HeaderValue>) {
    headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
    for value in upgrade {
        headers.append(UPGRADE, value);
    }
}

/// What the proxy knows about the client side of a request.
#[derive(Debug, Clone)]
pub(crate) struct Forwarded<'a> {
    pub client_ip: IpAddr,
    pub tls: bool,
    /// The `Host` (or HTTP/2 `:authority`) the client used, including any port.
    pub host: &'a str,
    /// `host` came from the request URI (HTTP/2 `:authority` or an HTTP/1
    /// absolute-form target) and must replace any `Host` header.
    pub replace_host: bool,
    /// HTTP/2 request: split `cookie` fields must be joined for HTTP/1.1.
    pub http2: bool,
    /// Protocol upgrade to keep (see [`requested_upgrade`]).
    pub upgrade: Option<Vec<HeaderValue>>,
}

/// Build the upstream request headers from the client's.
pub(crate) fn upstream_request_headers(mut headers: HeaderMap, fwd: Forwarded<'_>) -> HeaderMap {
    strip_hop_by_hop(&mut headers);
    // The proxy answers `100 Continue` itself (hyper does, once the body is
    // read) and streams the body right away; don't make the upstream wait too.
    headers.remove(EXPECT);
    if let Some(upgrade) = fwd.upgrade {
        restore_upgrade(&mut headers, upgrade);
    }

    let host_value = HeaderValue::from_str(fwd.host).ok();
    if (fwd.replace_host || !headers.contains_key(HOST))
        && let Some(v) = &host_value
    {
        headers.insert(HOST, v.clone());
    }
    if fwd.http2 {
        join_cookies(&mut headers);
    }

    let ip = fwd.client_ip.to_canonical().to_string();
    let xff = {
        let previous: Vec<&str> = headers
            .get_all(&X_FORWARDED_FOR)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .collect();
        if previous.is_empty() { ip.clone() } else { format!("{}, {ip}", previous.join(", ")) }
    };
    insert_str(&mut headers, X_FORWARDED_FOR, &xff);
    headers.insert(X_FORWARDED_PROTO, HeaderValue::from_static(if fwd.tls { "https" } else { "http" }));
    match host_value {
        Some(v) => {
            headers.insert(X_FORWARDED_HOST, v);
        }
        None => {
            headers.remove(X_FORWARDED_HOST);
        }
    }
    let port = host_port(fwd.host).unwrap_or(if fwd.tls { 443 } else { 80 });
    headers.insert(X_FORWARDED_PORT, HeaderValue::from(port));
    insert_str(&mut headers, X_REAL_IP, &ip);
    if !headers.contains_key(&X_REQUEST_ID) {
        insert_str(&mut headers, X_REQUEST_ID, &ferry_core::ids::random_secret(32));
    }
    headers
}

/// Response headers sent back to the client (hop-by-hop removed; for a
/// `101 Switching Protocols`, the upstream's `Upgrade` is kept).
pub(crate) fn client_response_headers(headers: &mut HeaderMap, switching_protocols: bool) {
    let upgrade: Vec<HeaderValue> =
        if switching_protocols { headers.get_all(UPGRADE).iter().cloned().collect() } else { Vec::new() };
    strip_hop_by_hop(headers);
    if switching_protocols {
        restore_upgrade(headers, upgrade);
    }
}

fn insert_str(headers: &mut HeaderMap, name: HeaderName, value: &str) {
    // All callers pass ASCII built from IPs, ids or existing header values.
    if let Ok(v) = HeaderValue::from_str(value) {
        headers.insert(name, v);
    }
}

/// HTTP/2 allows `cookie` to be split into several fields; HTTP/1.1 requires
/// a single one joined with "; " (RFC 9113 §8.2.3).
fn join_cookies(headers: &mut HeaderMap) {
    if headers.get_all(COOKIE).iter().nth(1).is_none() {
        return;
    }
    let joined: Vec<u8> =
        headers.get_all(COOKIE).iter().map(HeaderValue::as_bytes).collect::<Vec<_>>().join(&b"; "[..]);
    if let Ok(v) = HeaderValue::from_bytes(&joined) {
        headers.insert(COOKIE, v);
    }
}

/// Explicit port of a `host[:port]` / `[v6]:port` authority.
fn host_port(host: &str) -> Option<u16> {
    let after_name = match host.strip_prefix('[') {
        Some(rest) => &rest[rest.find(']')? + 1..],
        None => host,
    };
    let (_, port) = after_name.rsplit_once(':')?;
    port.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    fn fwd(host: &str) -> Forwarded<'_> {
        Forwarded {
            client_ip: "127.0.0.1".parse().unwrap(),
            tls: false,
            host,
            replace_host: false,
            http2: false,
            upgrade: None,
        }
    }

    #[test]
    fn strips_hop_by_hop_and_connection_listed() {
        let mut h = map(&[
            ("connection", "keep-alive, X-Custom"),
            ("connection", "x-other"),
            ("x-custom", "1"),
            ("x-other", "1"),
            ("keep-alive", "timeout=5"),
            ("proxy-connection", "keep-alive"),
            ("proxy-authorization", "Basic x"),
            ("proxy-authenticate", "Basic"),
            ("te", "trailers"),
            ("trailer", "x-t"),
            ("transfer-encoding", "chunked"),
            ("upgrade", "websocket"),
            ("x-kept", "yes"),
            ("content-length", "3"),
        ]);
        strip_hop_by_hop(&mut h);
        let mut names: Vec<&str> = h.keys().map(|k| k.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["content-length", "x-kept"]);
    }

    #[test]
    fn detects_upgrades() {
        let v11 = http::Version::HTTP_11;
        let ws = map(&[("connection", "keep-alive, Upgrade"), ("upgrade", "websocket")]);
        assert_eq!(requested_upgrade(v11, &ws), Some(vec![HeaderValue::from_static("websocket")]));
        assert_eq!(requested_upgrade(http::Version::HTTP_2, &ws), None);
        assert_eq!(requested_upgrade(v11, &map(&[("upgrade", "websocket")])), None);
        assert_eq!(requested_upgrade(v11, &map(&[("connection", "upgrade")])), None);
        let h2c = map(&[("connection", "Upgrade, HTTP2-Settings"), ("upgrade", "h2c"), ("http2-settings", "AAA")]);
        assert_eq!(requested_upgrade(v11, &h2c), None);
    }

    #[test]
    fn builds_forwarded_headers() {
        let h = map(&[
            ("host", "App.Test:8080"),
            ("x-forwarded-for", "10.0.0.1"),
            ("x-forwarded-for", "10.0.0.2"),
            ("x-forwarded-proto", "https"),
            ("x-real-ip", "6.6.6.6"),
            ("connection", "close"),
            ("expect", "100-continue"),
        ]);
        let out = upstream_request_headers(h, fwd("App.Test:8080"));
        assert_eq!(out[HOST], "App.Test:8080");
        assert_eq!(out[X_FORWARDED_FOR], "10.0.0.1, 10.0.0.2, 127.0.0.1");
        assert_eq!(out[X_FORWARDED_PROTO], "http");
        assert_eq!(out[X_FORWARDED_HOST], "App.Test:8080");
        assert_eq!(out[X_FORWARDED_PORT], "8080");
        assert_eq!(out[X_REAL_IP], "127.0.0.1");
        assert_eq!(out[X_REQUEST_ID].len(), 32);
        assert!(out.get(CONNECTION).is_none());
        assert!(out.get(EXPECT).is_none());
    }

    #[test]
    fn keeps_existing_request_id_and_defaults_port() {
        let h = map(&[("host", "a.test"), ("x-request-id", "abc")]);
        let mut f = fwd("a.test");
        f.tls = true;
        f.client_ip = "::ffff:10.1.2.3".parse().unwrap();
        let out = upstream_request_headers(h, f);
        assert_eq!(out[X_REQUEST_ID], "abc");
        assert_eq!(out[X_FORWARDED_PORT], "443");
        assert_eq!(out[X_FORWARDED_PROTO], "https");
        assert_eq!(out[X_FORWARDED_FOR], "10.1.2.3");
    }

    #[test]
    fn http2_requests_get_host_and_joined_cookies() {
        let h = map(&[("cookie", "a=1"), ("cookie", "b=2"), ("host", "stale.test")]);
        let mut f = fwd("h2.test:9443");
        f.http2 = true;
        f.replace_host = true;
        let out = upstream_request_headers(h, f);
        assert_eq!(out[HOST], "h2.test:9443");
        assert_eq!(out.get_all(COOKIE).iter().count(), 1);
        assert_eq!(out[COOKIE], "a=1; b=2");
    }

    #[test]
    fn upgrade_headers_survive() {
        let h = map(&[
            ("host", "ws.test"),
            ("connection", "Upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-key", "k"),
            ("keep-alive", "1"),
        ]);
        let mut f = fwd("ws.test");
        f.upgrade = requested_upgrade(http::Version::HTTP_11, &h);
        let out = upstream_request_headers(h, f);
        assert_eq!(out[CONNECTION], "upgrade");
        assert_eq!(out[UPGRADE], "websocket");
        assert_eq!(out["sec-websocket-key"], "k");
        assert!(out.get("keep-alive").is_none());

        let mut resp = map(&[("connection", "Upgrade"), ("upgrade", "websocket"), ("sec-websocket-accept", "a")]);
        client_response_headers(&mut resp, true);
        assert_eq!(resp[CONNECTION], "upgrade");
        assert_eq!(resp[UPGRADE], "websocket");
        let mut resp = map(&[("connection", "x-hop"), ("x-hop", "1"), ("transfer-encoding", "chunked"), ("x-a", "1")]);
        client_response_headers(&mut resp, false);
        assert_eq!(resp.keys().map(|k| k.as_str()).collect::<Vec<_>>(), vec!["x-a"]);
    }

    #[test]
    fn parses_host_ports() {
        assert_eq!(host_port("a.test:8080"), Some(8080));
        assert_eq!(host_port("a.test"), None);
        assert_eq!(host_port("[::1]:9000"), Some(9000));
        assert_eq!(host_port("[::1]"), None);
        assert_eq!(host_port("a.test:"), None);
    }
}
