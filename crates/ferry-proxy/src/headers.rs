//! Header rewriting between client and upstream: hop-by-hop stripping,
//! message framing, upgrade detection and `Forwarded` / `X-Forwarded-*`.

use std::net::IpAddr;

use http::header::{
    CONNECTION, CONTENT_LENGTH, COOKIE, EXPECT, FORWARDED, HOST, HeaderMap, HeaderName, HeaderValue,
    PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};

pub(crate) const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
pub(crate) const X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");
pub(crate) const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");
pub(crate) const X_FORWARDED_PORT: HeaderName = HeaderName::from_static("x-forwarded-port");
pub(crate) const X_REAL_IP: HeaderName = HeaderName::from_static("x-real-ip");
pub(crate) const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

const KEEP_ALIVE: HeaderName = HeaderName::from_static("keep-alive");
const PROXY_CONNECTION: HeaderName = HeaderName::from_static("proxy-connection");

/// Legacy/vendor headers claiming facts about earlier proxy hops (client
/// address, scheme), dropped from client requests along with `Forwarded`,
/// `X-Real-IP` and the whole `X-Forwarded-*` family (see
/// [`is_client_forwarding_header`]).
const CLIENT_FORWARDING_HEADERS: [&str; 5] =
    ["forwarded-for", "front-end-https", "x-url-scheme", "x-client-ip", "x-cluster-client-ip"];

/// Whether a client request header claims facts about earlier proxy hops
/// (client address, scheme, host, path prefix, client certificate) and must
/// be dropped before forwarding. Ferry is the edge, so such headers can only
/// come from the client itself, and frameworks trust some of them before
/// `X-Forwarded-Proto`/`-Host` (Rack and Spring read `Forwarded`; Rack honours
/// `X-Forwarded-Ssl: on`; Node's `request-ip` reads `X-Client-IP` first).
/// `Forwarded`, `X-Forwarded-{For,Proto,Host,Port}` and `X-Real-IP` are then
/// set from what the proxy saw. CDN-specific headers (`CF-Connecting-IP`,
/// `True-Client-IP`…) are left alone: apps reading them opt into that CDN.
fn is_client_forwarding_header(name: &HeaderName) -> bool {
    let name = name.as_str();
    name == FORWARDED.as_str()
        || name == X_REAL_IP.as_str()
        || name == "x-forwarded"
        || name.starts_with("x-forwarded-")
        || CLIENT_FORWARDING_HEADERS.contains(&name)
}

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

/// `Upgrade` values to forward if this HTTP/1.1 request asks for a protocol
/// upgrade we can tunnel (e.g. `websocket`), `None` otherwise. `h2c` upgrades
/// are not tunnelled: the request is proxied as plain HTTP/1.1. Neither are
/// HTTP/1.0 ones: `Upgrade` is HTTP/1.1-only (RFC 9110 §7.8) and hyper never
/// hands over an HTTP/1.0 connection, so the headers are dropped as
/// hop-by-hop instead of making the app switch protocols for nothing.
pub(crate) fn requested_upgrade(version: http::Version, headers: &HeaderMap) -> Option<Vec<HeaderValue>> {
    if version != http::Version::HTTP_11 {
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
    drop_length_if_chunked(&mut headers);
    strip_hop_by_hop(&mut headers);
    let spoofable: Vec<HeaderName> = headers.keys().filter(|name| is_client_forwarding_header(name)).cloned().collect();
    for name in spoofable {
        headers.remove(name);
    }
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

    // The client's own address only: as the edge, the proxy cannot tell a
    // genuine client-supplied `X-Forwarded-For` chain from a forged one.
    let client_ip = fwd.client_ip.to_canonical();
    let ip = client_ip.to_string();
    let proto = if fwd.tls { "https" } else { "http" };
    insert_str(&mut headers, X_FORWARDED_FOR, &ip);
    headers.insert(X_FORWARDED_PROTO, HeaderValue::from_static(proto));
    if let Some(v) = &host_value {
        headers.insert(X_FORWARDED_HOST, v.clone());
    }
    // The port of the URL the client used — the `Host` port, else the
    // scheme's default — consistent with `X-Forwarded-Host`. Not the
    // listener's port: behind port forwarding (router NAT, 443 → 8443) that
    // is not the port clients connect to, and apps would build broken URLs.
    let port = host_port(fwd.host).unwrap_or(if fwd.tls { 443 } else { 80 });
    headers.insert(X_FORWARDED_PORT, HeaderValue::from(port));
    insert_str(&mut headers, X_REAL_IP, &ip);
    insert_str(&mut headers, FORWARDED, &forwarded_element(client_ip, host_value.as_ref(), proto));
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
    drop_length_if_chunked(headers);
    strip_hop_by_hop(headers);
    if switching_protocols {
        restore_upgrade(headers, upgrade);
    }
}

/// A message with both `Transfer-Encoding` and `Content-Length` is framed by
/// the former, and an intermediary must drop the latter before forwarding
/// (RFC 9112 §6.3): the body is re-framed on the other side, and a stale
/// length would truncate it.
fn drop_length_if_chunked(headers: &mut HeaderMap) {
    if headers.contains_key(TRANSFER_ENCODING) {
        headers.remove(CONTENT_LENGTH);
    }
}

/// Make hyper's client send the request body chunked (for bodies of unknown
/// length; see `handler::forward`).
pub(crate) fn set_chunked(headers: &mut HeaderMap) {
    headers.remove(CONTENT_LENGTH);
    headers.insert(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
}

/// `for=…;host=…;proto=…` (RFC 7239) for this hop.
fn forwarded_element(client_ip: IpAddr, host: Option<&HeaderValue>, proto: &str) -> String {
    let mut out = match client_ip {
        IpAddr::V4(v4) => format!("for={v4}"),
        IpAddr::V6(v6) => format!("for=\"[{v6}]\""),
    };
    if let Some(host) = host.and_then(|h| h.to_str().ok()) {
        out.push_str(";host=");
        out.push_str(&forwarded_value(host));
    }
    out.push_str(";proto=");
    out.push_str(proto);
    out
}

/// An RFC 7239 value: a token as is, anything else as a quoted-string.
fn forwarded_value(value: &str) -> String {
    let is_tchar = |b: u8| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b);
    if !value.is_empty() && value.bytes().all(is_tchar) {
        return value.to_string();
    }
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for c in value.chars() {
        if c == '"' || c == '\\' {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
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
        // HTTP/1.0 has no upgrade mechanism.
        assert_eq!(requested_upgrade(http::Version::HTTP_10, &ws), None);
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
        // The edge only vouches for the address it saw.
        assert_eq!(out.get_all(X_FORWARDED_FOR).iter().collect::<Vec<_>>(), ["127.0.0.1"]);
        assert_eq!(out[X_FORWARDED_PROTO], "http");
        assert_eq!(out[X_FORWARDED_HOST], "App.Test:8080");
        assert_eq!(out[X_FORWARDED_PORT], "8080");
        assert_eq!(out[X_REAL_IP], "127.0.0.1");
        assert_eq!(out[FORWARDED], "for=127.0.0.1;host=\"App.Test:8080\";proto=http");
        assert_eq!(out[X_REQUEST_ID].len(), 32);
        assert!(out.get(CONNECTION).is_none());
        assert!(out.get(EXPECT).is_none());
    }

    #[test]
    fn drops_client_forwarding_headers() {
        let h = map(&[
            ("host", "app.test"),
            ("forwarded", "for=6.6.6.6;proto=https;host=evil.example"),
            ("forwarded", "for=7.7.7.7"),
            ("x-forwarded-ssl", "on"),
            ("x-forwarded-scheme", "https"),
            ("x-forwarded-protocol", "ssl"),
            ("x-forwarded-prefix", "//evil.example"),
            ("x-forwarded-server", "evil"),
            ("x-forwarded-client-cert", "Hash=abc;Subject=\"CN=admin\""),
            ("x-forwarded-host", "evil.example"),
            ("x-forwarded", "for=6.6.6.6"),
            ("forwarded-for", "6.6.6.6"),
            ("front-end-https", "on"),
            ("x-url-scheme", "https"),
            ("x-client-ip", "6.6.6.6"),
            ("x-cluster-client-ip", "6.6.6.6"),
            ("x-forwardedness", "not a forwarding header"),
            ("cf-connecting-ip", "198.51.100.9"),
        ]);
        let mut f = fwd("app.test");
        f.client_ip = "2001:db8::7".parse().unwrap();
        f.tls = true;
        let out = upstream_request_headers(h, f);
        let mut names: Vec<&str> = out.keys().map(HeaderName::as_str).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "cf-connecting-ip",
                "forwarded",
                "host",
                "x-forwarded-for",
                "x-forwarded-host",
                "x-forwarded-port",
                "x-forwarded-proto",
                "x-forwardedness",
                "x-real-ip",
                "x-request-id",
            ]
        );
        assert_eq!(
            out.get_all(FORWARDED).iter().collect::<Vec<_>>(),
            ["for=\"[2001:db8::7]\";host=app.test;proto=https"]
        );
        assert_eq!(out[X_FORWARDED_HOST], "app.test");
        assert_eq!(out[X_FORWARDED_FOR], "2001:db8::7");
        assert_eq!(out[X_FORWARDED_PORT], "443");
    }

    #[test]
    fn quotes_forwarded_values() {
        assert_eq!(forwarded_value("app.test"), "app.test");
        assert_eq!(forwarded_value("a&b'c.test"), "a&b'c.test");
        assert_eq!(forwarded_value("[::1]:8080"), "\"[::1]:8080\"");
        assert_eq!(forwarded_value("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(forwarded_value(""), "\"\"");
    }

    #[test]
    fn stale_content_length_is_dropped_when_chunked() {
        // Upstream response framed by Transfer-Encoding: the length is stale.
        let mut resp = map(&[("content-length", "3"), ("transfer-encoding", "chunked"), ("x-a", "1")]);
        client_response_headers(&mut resp, false);
        assert_eq!(resp.keys().map(|k| k.as_str()).collect::<Vec<_>>(), ["x-a"]);
        // Without Transfer-Encoding the length is kept.
        let mut resp = map(&[("content-length", "3")]);
        client_response_headers(&mut resp, false);
        assert_eq!(resp[CONTENT_LENGTH], "3");
        // Same for requests (hyper's server already drops it; defence in depth).
        let out =
            upstream_request_headers(map(&[("content-length", "3"), ("transfer-encoding", "chunked")]), fwd("a.test"));
        assert!(out.get(CONTENT_LENGTH).is_none() && out.get(TRANSFER_ENCODING).is_none());
        let mut h = map(&[("content-length", "3")]);
        set_chunked(&mut h);
        assert!(h.get(CONTENT_LENGTH).is_none());
        assert_eq!(h[TRANSFER_ENCODING], "chunked");
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
