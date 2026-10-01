//! Domains services are served under (DESIGN.md §21): what the API, the
//! engine and the proxy share about them.
//!
//! A domain is **verified** with a request the server sends to itself the
//! way a visitor would: `GET http://<a made-up name under the domain>/
//! .well-known/ferry-domain-check/<token>`. A name nobody used before is
//! only answered by a wildcard DNS record — and no resolver has an old
//! answer for it in its cache — and only this server's proxy answers
//! `<token>.<its probe id>` ([`probe_answer`]).

use std::net::IpAddr;

use crate::dto::DnsRecord;
use crate::{Config, Error, Result, Store, ids, validate};

/// Connected domains per server: each one adds a hostname to every service.
pub const MAX_DOMAINS: usize = 20;

/// Path of a verification request, followed by its token.
pub const PROBE_PATH: &str = "/.well-known/ferry-domain-check/";

/// What the proxy of the server with this probe id answers a verification
/// request with.
pub fn probe_answer(token: &str, probe_id: &str) -> String {
    format!("{token}.{probe_id}")
}

/// Whether `token` is one the proxy echoes: letters and digits only, 64 at
/// most — nothing an answer could be made to say.
pub fn is_probe_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 64 && token.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The token of one verification request.
pub fn new_probe_token() -> String {
    ids::random_secret(24)
}

/// A name under `domain` that no zone has a record of its own for: only a
/// wildcard record answers for it.
pub fn probe_host(domain: &str) -> String {
    format!("ferry-check-{}.{domain}", ids::random_secret(10))
}

/// The name of a domain to connect: lowercase, without a trailing dot, and
/// a domain name — not a wildcard, a URL or an address.
pub fn connectable_name(input: &str) -> Result<String> {
    let name = input.trim().trim_end_matches('.').to_ascii_lowercase();
    if let Some(rest) = name.strip_prefix("*.") {
        return Err(Error::invalid(format!("enter the domain without '*.': services are served at <service>.{rest}")));
    }
    if name.contains("://") || name.contains('/') {
        return Err(Error::invalid(format!("'{name}' is a URL: enter the domain alone, like example.com")));
    }
    if name.parse::<IpAddr>().is_ok() {
        return Err(Error::invalid(format!("'{name}' is an IP address: enter a domain name, like example.com")));
    }
    validate::domain(&name)
}

/// Read the domains of the store into `config.domains`, which every part of
/// the server derives the hostnames of a service from. Returns whether what
/// is served changed.
pub async fn reload(store: &Store, config: &Config) -> Result<bool> {
    let domains = store.list_domains().await?;
    Ok(config.domains.replace(&config.base_domain, &domains))
}

/// What a server does with its domains when it starts: keep the row of the
/// base domain in step with `--base-domain`, then [`reload`].
pub async fn init(store: &Store, config: &Config) -> Result<()> {
    store.sync_base_domain(&config.base_domain).await?;
    reload(store, config).await?;
    Ok(())
}

/// The DNS records that point a domain at a server reached at `addresses`:
/// the wildcard that covers every service, then the domain itself (only
/// needed to serve something there). One wildcard record is required — the
/// IPv4 one when the server has an IPv4 address: IPv6 comes on top. Without
/// a known address the records come without a value.
pub fn dns_records(addresses: &[IpAddr]) -> Vec<DnsRecord> {
    let has_v4 = addresses.iter().any(IpAddr::is_ipv4);
    let record = |name: &str, ip: Option<&IpAddr>| {
        let v6 = matches!(ip, Some(IpAddr::V6(_)));
        DnsRecord {
            record_type: if v6 { "AAAA" } else { "A" }.to_string(),
            name: name.to_string(),
            value: ip.map(IpAddr::to_string),
            required: name == "*" && !(v6 && has_v4),
        }
    };
    let mut out = Vec::new();
    for name in ["*", "@"] {
        if addresses.is_empty() {
            out.push(record(name, None));
        }
        // IPv4 first: it is the record every DNS provider asks for.
        for ip in addresses.iter().filter(|ip| ip.is_ipv4()).chain(addresses.iter().filter(|ip| ip.is_ipv6())) {
            out.push(record(name, Some(ip)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Domain, DomainSource};

    #[test]
    fn probes() {
        let host = probe_host("example.com");
        assert!(host.starts_with("ferry-check-") && host.ends_with(".example.com"), "{host}");
        assert_ne!(host, probe_host("example.com"));
        assert!(validate::domain(&host).is_ok());
        let token = new_probe_token();
        assert!(is_probe_token(&token));
        assert_eq!(probe_answer("abc", "id1"), "abc.id1");
        for bad in ["", "a b", "a/b", "a.b", "<script>", &"a".repeat(65)] {
            assert!(!is_probe_token(bad), "{bad}");
        }
    }

    #[test]
    fn names() {
        assert_eq!(connectable_name(" Example.COM. ").unwrap(), "example.com");
        assert_eq!(connectable_name("apps.example.com").unwrap(), "apps.example.com");
        assert_eq!(connectable_name("dev.localhost").unwrap(), "dev.localhost");
        let err = connectable_name("*.example.com").unwrap_err().to_string();
        assert!(err.contains("<service>.example.com"), "{err}");
        for bad in ["", "localhost", "https://example.com", "example.com/app", "exa mple.com", "203.0.113.10", "-a.com"]
        {
            assert!(connectable_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn records() {
        let v4: IpAddr = "203.0.113.10".parse().unwrap();
        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        let rows: Vec<(String, String, Option<String>, bool)> =
            dns_records(&[v6, v4]).into_iter().map(|r| (r.record_type, r.name, r.value, r.required)).collect();
        assert_eq!(
            rows,
            vec![
                ("A".into(), "*".into(), Some("203.0.113.10".into()), true),
                ("AAAA".into(), "*".into(), Some("2001:db8::1".into()), false),
                ("A".into(), "@".into(), Some("203.0.113.10".into()), false),
                ("AAAA".into(), "@".into(), Some("2001:db8::1".into()), false),
            ]
        );
        // A server with an IPv6 address only: that record is the one to create.
        let v6_only = dns_records(&[v6]);
        assert_eq!((v6_only[0].record_type.as_str(), v6_only[0].required), ("AAAA", true));
        let unknown = dns_records(&[]);
        assert_eq!(unknown.len(), 2);
        assert!(unknown.iter().all(|r| r.record_type == "A" && r.value.is_none()));
        assert!(unknown[0].required && !unknown[1].required);
    }

    #[tokio::test]
    async fn reload_reads_the_store() {
        let store = Store::open_in_memory().await.unwrap();
        let config = Config::default();
        store.sync_base_domain(&config.base_domain).await.unwrap();
        assert!(reload(&store, &config).await.unwrap());
        assert!(!reload(&store, &config).await.unwrap());
        assert_eq!(config.claimed_domains(), vec!["localhost"]);
        store.create_domain(&Domain::new("example.com", DomainSource::Connected)).await.unwrap();
        assert!(reload(&store, &config).await.unwrap());
        assert_eq!(config.served_domains(), vec!["localhost"]);
        assert_eq!(config.claimed_domains(), vec!["localhost", "example.com"]);
    }
}
