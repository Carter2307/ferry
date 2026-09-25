//! Runtime detection and small parsers for project manifests (Procfile,
//! package.json engines, go.mod, Cargo.toml, version files).

use std::fs;
use std::path::Path;

use ferry_core::{Runtime, ServiceType};

/// See [`crate::detect_runtime`].
pub(crate) fn detect(dir: &Path) -> Option<Runtime> {
    let has = |name: &str| dir.join(name).is_file();
    if has("Dockerfile") {
        Some(Runtime::Docker)
    } else if has("package.json") || has("bun.lockb") || has("bun.lock") {
        Some(Runtime::Node)
    } else if has("requirements.txt") || has("pyproject.toml") || has("Pipfile") {
        Some(Runtime::Python)
    } else if has("go.mod") {
        Some(Runtime::Go)
    } else if has("Cargo.toml") {
        Some(Runtime::Rust)
    } else if has("Gemfile") {
        Some(Runtime::Ruby)
    } else if has("index.html") {
        Some(Runtime::Static)
    } else {
        None
    }
}

/// Read a small text file of the project, `None` if absent or unreadable.
pub(crate) fn read_text(dir: &Path, name: &str) -> Option<String> {
    let path = dir.join(name);
    let meta = fs::metadata(&path).ok()?;
    // Manifests are small; refuse to slurp anything huge.
    if !meta.is_file() || meta.len() > 8 * 1024 * 1024 {
        return None;
    }
    fs::read(&path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Parse a Procfile into `(process type, command)` pairs, in file order.
pub(crate) fn parse_procfile(contents: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, cmd)) = line.split_once(':') else { continue };
        let name = name.trim();
        let cmd = cmd.trim();
        if name.is_empty() || cmd.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            continue;
        }
        // Later duplicates override earlier ones, like Heroku.
        out.retain(|(n, _)| n != name);
        out.push((name.to_string(), cmd.to_string()));
    }
    out
}

/// The Procfile entry to use for a service type: `web` for services that
/// listen, `worker` for background workers, `cron`/`worker` for cron jobs.
pub(crate) fn procfile_command(dir: &Path, service_type: Option<ServiceType>) -> Option<String> {
    let procs = parse_procfile(&read_text(dir, "Procfile")?);
    let wanted: &[&str] = match service_type {
        Some(ServiceType::BackgroundWorker) => &["worker"],
        Some(ServiceType::CronJob) => &["cron", "worker"],
        _ => &["web"],
    };
    wanted.iter().find_map(|w| procs.iter().find(|(n, _)| n == w).map(|(_, c)| c.clone()))
}

/// Node.js major versions considered when resolving `engines.node`.
const NODE_MAJORS: &[u32] = &[18, 20, 22, 24];
pub(crate) const DEFAULT_NODE_MAJOR: u32 = 22;

/// Pick a Node major for an `engines.node` range: the default (22) when it
/// satisfies the range, else the newest known major that does, else the first
/// major mentioned, else the default.
pub(crate) fn node_major_for(range: Option<&str>) -> u32 {
    let Some(range) = range.map(str::trim).filter(|r| !r.is_empty()) else {
        return DEFAULT_NODE_MAJOR;
    };
    let mentioned = mentioned_majors(range);
    let mut candidates: Vec<u32> = NODE_MAJORS.to_vec();
    candidates.extend(mentioned.iter().copied().filter(|m| (4..=99).contains(m)));
    candidates.sort_unstable();
    candidates.dedup();
    if range_allows(range, DEFAULT_NODE_MAJOR) {
        return DEFAULT_NODE_MAJOR;
    }
    if let Some(best) = candidates.iter().rev().find(|m| range_allows(range, **m)) {
        return *best;
    }
    mentioned.first().copied().filter(|m| (4..=99).contains(m)).unwrap_or(DEFAULT_NODE_MAJOR)
}

fn mentioned_majors(range: &str) -> Vec<u32> {
    range
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter_map(|tok| tok.split('.').next().and_then(|m| m.parse().ok()))
        .collect()
}

/// Does the semver-ish `range` accept some version with major `m`?
/// Supports `||`, space-separated comparators, `^ ~ >= > <= < =`, `x`
/// wildcards and hyphen ranges; minor/patch parts are approximated.
fn range_allows(range: &str, m: u32) -> bool {
    range.split("||").any(|alt| alternative_allows(alt.trim(), m))
}

fn alternative_allows(alt: &str, m: u32) -> bool {
    if alt.is_empty() || matches!(alt, "*" | "x" | "X" | "latest" | "lts/*" | "node" | "current") {
        return true;
    }
    if let Some((lo, hi)) = alt.split_once(" - ") {
        let (Some(lo), Some(hi)) = (parse_version(lo.trim()), parse_version(hi.trim())) else { return false };
        return m >= lo.0 && m <= hi.0;
    }
    // Join operators separated from their version by spaces (">= 18").
    let mut tokens: Vec<String> = Vec::new();
    for t in alt.split_whitespace() {
        if let Some(last) = tokens.last_mut()
            && matches!(last.as_str(), ">" | ">=" | "<" | "<=" | "=" | "^" | "~")
        {
            last.push_str(t);
            continue;
        }
        tokens.push(t.to_string());
    }
    tokens.iter().all(|t| comparator_allows(t, m))
}

fn comparator_allows(t: &str, m: u32) -> bool {
    let (op, ver) = if let Some(v) = t.strip_prefix(">=") {
        (">=", v)
    } else if let Some(v) = t.strip_prefix("<=") {
        ("<=", v)
    } else if let Some(v) = t.strip_prefix('>') {
        (">", v)
    } else if let Some(v) = t.strip_prefix('<') {
        ("<", v)
    } else if let Some(v) = t.strip_prefix('=') {
        ("=", v)
    } else if let Some(v) = t.strip_prefix('^') {
        ("=", v)
    } else if let Some(v) = t.strip_prefix('~') {
        ("=", v)
    } else {
        ("=", t)
    };
    let ver = ver.trim().trim_start_matches('v');
    if matches!(ver, "*" | "x" | "X" | "") {
        return true;
    }
    let Some((major, rest_nonzero)) = parse_version(ver) else { return true };
    match op {
        ">=" => m >= major,
        ">" => {
            if rest_nonzero || ver.contains('.') {
                m >= major
            } else {
                m > major
            }
        }
        "<=" => m <= major,
        "<" => {
            if rest_nonzero {
                m <= major
            } else {
                m < major
            }
        }
        _ => m == major,
    }
}

/// `"20.10.1"` → `(20, true)`; `"22"` / `"22.x"` / `"22.0.0"` → `(22, false)`.
fn parse_version(v: &str) -> Option<(u32, bool)> {
    let v = v.trim().trim_start_matches('v');
    let mut parts = v.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let rest_nonzero = parts.any(|p| p.parse::<u32>().is_ok_and(|n| n > 0));
    Some((major, rest_nonzero))
}

/// `3.11.4` / `python-3.11.4` / `3.11` → `"3.11"`.
pub(crate) fn python_version(dir: &Path) -> Option<String> {
    if let Some(v) = read_text(dir, ".python-version")
        && let Some(line) = v.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'))
        && let Some(mm) = major_minor(line)
    {
        return Some(mm);
    }
    let rt = read_text(dir, "runtime.txt")?;
    let line = rt.lines().map(str::trim).find(|l| !l.is_empty())?;
    major_minor(line.strip_prefix("python-")?)
}

/// `"3.12.1"` → `"3.12"`; requires `major.minor` digits.
pub(crate) fn major_minor(v: &str) -> Option<String> {
    let v = v.trim().trim_start_matches('v');
    let mut parts = v.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let digits = |s: &str| !s.is_empty() && s.len() <= 3 && s.chars().all(|c| c.is_ascii_digit());
    let minor: String = minor.chars().take_while(|c| c.is_ascii_digit()).collect();
    (digits(major) && digits(&minor)).then(|| format!("{major}.{minor}"))
}

/// Ruby `major.minor` from `.ruby-version` (`3.2.2`, `ruby-3.2.2`) or the
/// Gemfile's `ruby "3.2.2"` directive.
pub(crate) fn ruby_version(dir: &Path) -> Option<String> {
    if let Some(v) = read_text(dir, ".ruby-version")
        && let Some(line) = v.lines().map(str::trim).find(|l| !l.is_empty())
        && let Some(mm) = major_minor(line.trim_start_matches("ruby-"))
    {
        return Some(mm);
    }
    let gemfile = read_text(dir, "Gemfile")?;
    gemfile.lines().map(str::trim).find_map(|l| {
        let rest = l.strip_prefix("ruby ")?;
        let quoted = rest.trim().trim_start_matches(['\'', '"']);
        let ver: String = quoted.chars().skip_while(|c| !c.is_ascii_digit()).collect();
        major_minor(&ver)
    })
}

/// Go `major.minor` from the `go 1.xx[.y]` directive of go.mod.
pub(crate) fn go_version(dir: &Path) -> Option<String> {
    let gomod = read_text(dir, "go.mod")?;
    gomod.lines().map(str::trim).find_map(|l| {
        let v = l.strip_prefix("go ")?.trim();
        major_minor(v)
    })
}

/// Binary information extracted from Cargo.toml.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CargoInfo {
    pub package_name: Option<String>,
    pub default_run: Option<String>,
    pub bins: Vec<String>,
    pub is_workspace: bool,
}

/// Minimal Cargo.toml reader: `[package] name / default-run`, `[[bin]] name`
/// and whether a `[workspace]` table exists. Good enough for choosing the
/// binary; not a general TOML parser.
pub(crate) fn parse_cargo_toml(contents: &str) -> CargoInfo {
    #[derive(PartialEq)]
    enum Section {
        Package,
        Bin,
        Other,
    }
    let mut info = CargoInfo::default();
    let mut section = Section::Other;
    for raw in contents.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("[[") {
            let name = line.trim_start_matches("[[").trim_end_matches("]]").trim();
            section = if name == "bin" { Section::Bin } else { Section::Other };
            continue;
        }
        if line.starts_with('[') {
            let name = line.trim_start_matches('[').trim_end_matches(']').trim();
            if name == "workspace" || name.starts_with("workspace.") {
                info.is_workspace = true;
            }
            section = if name == "package" { Section::Package } else { Section::Other };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        let value = value.trim();
        let unquoted = |v: &str| -> Option<String> {
            let v = v.trim();
            let inner = v
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))?;
            (!inner.is_empty()).then(|| inner.to_string())
        };
        match (&section, key) {
            (Section::Package, "name") => info.package_name = unquoted(value),
            (Section::Package, "default-run") => info.default_run = unquoted(value),
            (Section::Bin, "name") => {
                if let Some(n) = unquoted(value) {
                    info.bins.push(n);
                }
            }
            _ => {}
        }
    }
    info
}

/// The binary a Rust project produces: `default-run`, else the package name
/// when `src/main.rs` exists (or there are no `[[bin]]` targets), else the
/// first `[[bin]]`, else the first `src/bin/*.rs`.
pub(crate) fn rust_binary(dir: &Path, info: &CargoInfo) -> Option<String> {
    if let Some(d) = &info.default_run {
        return Some(d.clone());
    }
    let has_main = dir.join("src/main.rs").is_file();
    if let Some(pkg) = &info.package_name
        && (has_main || (info.bins.is_empty() && !dir.join("src/bin").is_dir()))
    {
        return Some(pkg.clone());
    }
    if let Some(b) = info.bins.first() {
        return Some(b.clone());
    }
    let mut autobins: Vec<String> = fs::read_dir(dir.join("src/bin"))
        .ok()?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "rs") {
                p.file_stem().map(|s| s.to_string_lossy().into_owned())
            } else if p.join("main.rs").is_file() {
                p.file_name().map(|s| s.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect();
    autobins.sort();
    autobins.into_iter().next().or_else(|| info.package_name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str, contents: &str) {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, contents).unwrap();
    }

    #[test]
    fn detection_order() {
        let cases: &[(&[&str], Option<Runtime>)] = &[
            (&["Dockerfile", "package.json"], Some(Runtime::Docker)),
            (&["package.json", "requirements.txt"], Some(Runtime::Node)),
            (&["bun.lockb"], Some(Runtime::Node)),
            (&["bun.lock"], Some(Runtime::Node)),
            (&["requirements.txt"], Some(Runtime::Python)),
            (&["pyproject.toml"], Some(Runtime::Python)),
            (&["Pipfile"], Some(Runtime::Python)),
            (&["go.mod", "index.html"], Some(Runtime::Go)),
            (&["Cargo.toml"], Some(Runtime::Rust)),
            (&["Gemfile"], Some(Runtime::Ruby)),
            (&["index.html"], Some(Runtime::Static)),
            (&["README.md"], None),
        ];
        for (files, expected) in cases {
            let d = tempfile::tempdir().unwrap();
            for f in *files {
                touch(d.path(), f, "");
            }
            assert_eq!(detect(d.path()), *expected, "{files:?}");
        }
        // A directory named like a manifest does not count.
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("Dockerfile")).unwrap();
        assert_eq!(detect(d.path()), None);
    }

    #[test]
    fn procfile_parsing() {
        let p = parse_procfile(
            "# comment\nweb: gunicorn app:app --bind 0.0.0.0:$PORT\n\nworker:python worker.py\nbad line\n: nocmd\nrelease: \nweb: python app.py\n",
        );
        assert_eq!(
            p,
            vec![
                ("worker".to_string(), "python worker.py".to_string()),
                ("web".to_string(), "python app.py".to_string()),
            ]
        );
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "Procfile", "web: node web.js\nworker: node worker.js\ncron: node cron.js\n");
        assert_eq!(procfile_command(d.path(), Some(ServiceType::WebService)).unwrap(), "node web.js");
        assert_eq!(procfile_command(d.path(), Some(ServiceType::PrivateService)).unwrap(), "node web.js");
        assert_eq!(procfile_command(d.path(), None).unwrap(), "node web.js");
        assert_eq!(procfile_command(d.path(), Some(ServiceType::BackgroundWorker)).unwrap(), "node worker.js");
        assert_eq!(procfile_command(d.path(), Some(ServiceType::CronJob)).unwrap(), "node cron.js");
        let e = tempfile::tempdir().unwrap();
        assert!(procfile_command(e.path(), None).is_none());
    }

    #[test]
    fn node_versions() {
        assert_eq!(node_major_for(None), 22);
        assert_eq!(node_major_for(Some("18.x")), 18);
        assert_eq!(node_major_for(Some("^20.10.0")), 20);
        assert_eq!(node_major_for(Some(">=18")), 22);
        assert_eq!(node_major_for(Some(">= 18 < 21")), 20);
        assert_eq!(node_major_for(Some(">=24")), 24);
        assert_eq!(node_major_for(Some("16 || 18")), 18);
        assert_eq!(node_major_for(Some("20 - 22")), 22);
        assert_eq!(node_major_for(Some("~16.14")), 16);
        assert_eq!(node_major_for(Some("<20")), 18);
        assert_eq!(node_major_for(Some("lts/*")), 22);
        assert_eq!(node_major_for(Some("v21.1.0")), 21);
        assert_eq!(node_major_for(Some("nonsense")), 22);
    }

    #[test]
    fn version_files() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(python_version(d.path()), None);
        touch(d.path(), "runtime.txt", "python-3.11.4\n");
        assert_eq!(python_version(d.path()).unwrap(), "3.11");
        touch(d.path(), ".python-version", "3.10\n");
        assert_eq!(python_version(d.path()).unwrap(), "3.10");
        touch(d.path(), ".python-version", "pypy3.9\n");
        assert_eq!(python_version(d.path()).unwrap(), "3.11");

        touch(d.path(), "go.mod", "module example.com/x\n\ngo 1.22.3\n\ntoolchain go1.23.1\n");
        assert_eq!(go_version(d.path()).unwrap(), "1.22");
        touch(d.path(), "Gemfile", "source 'https://rubygems.org'\nruby '3.2.2'\n");
        assert_eq!(ruby_version(d.path()).unwrap(), "3.2");
        touch(d.path(), ".ruby-version", "ruby-3.1.4\n");
        assert_eq!(ruby_version(d.path()).unwrap(), "3.1");
        assert_eq!(major_minor("3"), None);
        assert_eq!(major_minor("1.21rc1").unwrap(), "1.21");
    }

    #[test]
    fn cargo_toml_parsing_and_binary_choice() {
        let info = parse_cargo_toml(
            "[package]\nname = \"my-app\" # comment\nversion = \"0.1.0\"\n\n[dependencies]\nname = \"not-this\"\n\n[[bin]]\nname = 'tool'\npath = \"src/tool.rs\"\n",
        );
        assert_eq!(info.package_name.as_deref(), Some("my-app"));
        assert_eq!(info.bins, vec!["tool"]);
        assert!(!info.is_workspace);

        let d = tempfile::tempdir().unwrap();
        // no src/main.rs but a [[bin]] → the bin
        assert_eq!(rust_binary(d.path(), &info).unwrap(), "tool");
        touch(d.path(), "src/main.rs", "fn main() {}");
        assert_eq!(rust_binary(d.path(), &info).unwrap(), "my-app");

        let dr = parse_cargo_toml("[package]\nname = \"a\"\ndefault-run = \"b\"\n");
        assert_eq!(rust_binary(d.path(), &dr).unwrap(), "b");

        let ws = parse_cargo_toml("[workspace]\nmembers = [\"api\"]\n");
        assert!(ws.is_workspace);
        assert_eq!(ws.package_name, None);
        let e = tempfile::tempdir().unwrap();
        assert_eq!(rust_binary(e.path(), &ws), None);

        let auto = tempfile::tempdir().unwrap();
        touch(auto.path(), "src/bin/server.rs", "fn main() {}");
        touch(auto.path(), "src/lib.rs", "");
        let only_pkg = parse_cargo_toml("[package]\nname = \"lib-and-bins\"\n");
        assert_eq!(rust_binary(auto.path(), &only_pkg).unwrap(), "server");
    }
}
