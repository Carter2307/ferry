//! Input validation shared by the API, blueprints and the engine.

use crate::models::{Runtime, Service, ServiceType};
use crate::schedule::Schedule;
use crate::{Error, Result};

/// Names that cannot be used for services/datastores.
pub const RESERVED_NAMES: &[&str] = &["ferry", "localhost", "api", "proxy"];

/// Maximum instances per service.
pub const MAX_INSTANCES: u32 = 50;

/// Service / datastore names: DNS label, 1–40 chars, `[a-z0-9-]`, starts with a
/// letter, doesn't end with `-`. They double as private-network hostnames.
pub fn resource_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 40
        && name.as_bytes()[0].is_ascii_lowercase()
        && !name.ends_with('-')
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !ok {
        return Err(Error::invalid(format!(
            "invalid name '{name}': use 1-40 lowercase letters, digits or '-', starting with a letter"
        )));
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(Error::invalid(format!("name '{name}' is reserved")));
    }
    Ok(())
}

/// Env group names: 1–64 chars of `[A-Za-z0-9_.-]`.
pub fn env_group_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if ok { Ok(()) } else { Err(Error::invalid(format!("invalid env group name '{name}'"))) }
}

/// Environment variable keys: non-empty, no `=`, whitespace or NUL, ≤ 256 chars.
pub fn env_key(key: &str) -> Result<()> {
    let ok = !key.is_empty() && key.len() <= 256 && !key.chars().any(|c| c == '=' || c == '\0' || c.is_whitespace());
    if ok { Ok(()) } else { Err(Error::invalid(format!("invalid environment variable name '{key}'"))) }
}

/// Validate and normalize (lowercase, no trailing dot) a custom domain.
pub fn domain(d: &str) -> Result<String> {
    let d = d.trim().trim_end_matches('.').to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    let ok = d.len() <= 253 && d.contains('.') && d.split('.').all(label_ok);
    if ok { Ok(d) } else { Err(Error::invalid(format!("invalid domain '{d}'"))) }
}

/// Absolute mount path other than `/`.
pub fn mount_path(p: &str) -> Result<()> {
    if p.starts_with('/') && p.len() > 1 && !p.contains("..") {
        Ok(())
    } else {
        Err(Error::invalid(format!("invalid disk mount path '{p}': must be an absolute path other than '/'")))
    }
}

/// HTTP path starting with `/`.
pub fn health_check_path(p: &str) -> Result<()> {
    if p.starts_with('/') && !p.chars().any(char::is_whitespace) {
        Ok(())
    } else {
        Err(Error::invalid(format!("invalid health check path '{p}': must start with '/'")))
    }
}

/// Check a whole service for internal consistency. Call after applying user
/// input and [`normalize_service`].
pub fn service(svc: &Service) -> Result<()> {
    resource_name(&svc.name)?;
    if svc.repo_url.is_some() && svc.image.is_some() {
        return Err(Error::invalid("set either repo_url or image, not both"));
    }
    if svc.runtime == Runtime::Image && svc.image.is_none() {
        return Err(Error::invalid("runtime 'image' requires an image"));
    }
    if let Some(r) = &svc.repo_url
        && (r.trim().is_empty() || r.chars().any(char::is_whitespace))
    {
        return Err(Error::invalid(format!("invalid repo_url '{r}'")));
    }
    if let Some(i) = &svc.image
        && (i.trim().is_empty() || i.chars().any(char::is_whitespace))
    {
        return Err(Error::invalid(format!("invalid image '{i}'")));
    }
    if svc.branch.trim().is_empty() || svc.branch.chars().any(char::is_whitespace) {
        return Err(Error::invalid(format!("invalid branch '{}'", svc.branch)));
    }
    match (svc.service_type, &svc.schedule) {
        (ServiceType::CronJob, Some(s)) => {
            Schedule::parse(s)?;
        }
        (ServiceType::CronJob, None) => return Err(Error::invalid("cron jobs require a schedule")),
        (_, Some(_)) => return Err(Error::invalid("only cron jobs can have a schedule")),
        _ => {}
    }
    if svc.instances == 0 || svc.instances > MAX_INSTANCES {
        return Err(Error::invalid(format!(
            "instances must be between 1 and {MAX_INSTANCES} (use suspend to stop a service)"
        )));
    }
    if let Some(p) = &svc.disk_mount_path {
        mount_path(p)?;
        if svc.instances > 1 {
            return Err(Error::invalid("services with a disk are limited to 1 instance"));
        }
        if svc.service_type == ServiceType::StaticSite || svc.service_type == ServiceType::CronJob {
            return Err(Error::invalid("disks are only supported on web services, private services and workers"));
        }
    }
    if !svc.custom_domains.is_empty() && !svc.is_public_http() {
        return Err(Error::invalid("custom domains are only supported on web services and static sites"));
    }
    for d in &svc.custom_domains {
        domain(d)?;
    }
    if let Some(p) = &svc.health_check_path {
        if !svc.listens() {
            return Err(Error::invalid("health checks are only supported on services that listen on a port"));
        }
        health_check_path(p)?;
    }
    if svc.port == Some(0) {
        return Err(Error::invalid("port must be between 1 and 65535"));
    }
    Ok(())
}

/// Normalize user input in place: trims strings, turns empty strings into
/// `None`, lowercases/dedups domains, and forces runtime `image` when an image
/// is set.
pub fn normalize_service(svc: &mut Service) {
    fn clean(o: &mut Option<String>) {
        if let Some(s) = o {
            let t = s.trim();
            if t.is_empty() {
                *o = None;
            } else if t.len() != s.len() {
                *o = Some(t.to_string());
            }
        }
    }
    for f in [
        &mut svc.repo_url,
        &mut svc.image,
        &mut svc.root_dir,
        &mut svc.dockerfile_path,
        &mut svc.build_command,
        &mut svc.start_command,
        &mut svc.publish_dir,
        &mut svc.health_check_path,
        &mut svc.schedule,
        &mut svc.disk_mount_path,
    ] {
        clean(f);
    }
    svc.name = svc.name.trim().to_string();
    svc.branch = svc.branch.trim().to_string();
    if svc.branch.is_empty() {
        svc.branch = "main".to_string();
    }
    if svc.image.is_some() {
        svc.runtime = Runtime::Image;
    } else if svc.runtime == Runtime::Image {
        // runtime image without an image is caught by `service()`.
    }
    let mut domains: Vec<String> = Vec::new();
    for d in &svc.custom_domains {
        let d = d.trim().trim_end_matches('.').to_ascii_lowercase();
        if !d.is_empty() && !domains.contains(&d) {
            domains.push(d);
        }
    }
    svc.custom_domains = domains;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(resource_name("web").is_ok());
        assert!(resource_name("my-api-2").is_ok());
        assert!(resource_name("2web").is_err());
        assert!(resource_name("Web").is_err());
        assert!(resource_name("web-").is_err());
        assert!(resource_name("ferry").is_err());
        assert!(resource_name(&"a".repeat(41)).is_err());
    }

    #[test]
    fn domains_and_keys() {
        assert_eq!(domain("App.Example.COM.").unwrap(), "app.example.com");
        assert!(domain("localhost").is_err());
        assert!(domain("-a.com").is_err());
        assert!(env_key("DATABASE_URL").is_ok());
        assert!(env_key("A=B").is_err());
        assert!(env_key("").is_err());
    }

    #[test]
    fn service_rules() {
        let mut s = Service::new("job", ServiceType::CronJob);
        assert!(service(&s).is_err());
        s.schedule = Some("*/5 * * * *".into());
        assert!(service(&s).is_ok());
        let mut w = Service::new("web", ServiceType::WebService);
        w.image = Some(" nginx:alpine ".into());
        w.custom_domains = vec!["A.com".into(), "a.com".into()];
        normalize_service(&mut w);
        assert_eq!(w.runtime, Runtime::Image);
        assert_eq!(w.image.as_deref(), Some("nginx:alpine"));
        assert_eq!(w.custom_domains, vec!["a.com"]);
        assert!(service(&w).is_ok());
        w.disk_mount_path = Some("/data".into());
        w.instances = 2;
        assert!(service(&w).is_err());
        let mut worker = Service::new("bg", ServiceType::BackgroundWorker);
        worker.custom_domains = vec!["x.com".into()];
        assert!(service(&worker).is_err());
    }
}
