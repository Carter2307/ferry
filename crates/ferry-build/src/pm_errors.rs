//! One-line explanations of failed npm / pnpm / yarn runs, for the build
//! error summary.
//!
//! The most useful line is rarely the last one: npm puts the explanation in
//! the middle of its error block and ends it with advice ("Note that you can
//! also install from a tarball, folder, http url, or git url.") and pointers
//! to log files; pnpm and yarn follow their error with context lines.

/// The explanation of a package-manager failure in a build step's output
/// (BuildKit timestamps already removed), or `None` when the output holds no
/// package-manager error worth more than the step's own output.
pub(crate) fn summarize(output: &[&str]) -> Option<String> {
    npm(output).or_else(|| pnpm(output)).or_else(|| yarn_berry(output)).or_else(|| yarn_classic(output))
}

// ---------------------------------------------------------------------------
// npm

/// Messages of `npm error …` / `npm ERR! …` lines, prefix removed.
fn npm_messages<'a>(output: &[&'a str]) -> Vec<&'a str> {
    output
        .iter()
        .filter_map(|l| {
            let l = l.trim();
            ["npm error", "npm err!"].iter().find_map(|p| {
                let head = l.get(..p.len())?;
                let rest = &l[p.len()..];
                (head.eq_ignore_ascii_case(p) && (rest.is_empty() || rest.starts_with(' '))).then(|| rest.trim())
            })
        })
        .collect()
}

/// npm tags some lines with a category (`npm error network request to …`,
/// `npm error notarget No matching version …`): drop it.
fn strip_tag(m: &str) -> &str {
    for tag in ["network ", "notarget ", "enoent "] {
        if let Some(rest) = m.strip_prefix(tag) {
            return rest.trim();
        }
    }
    m
}

fn trim_period(m: &str) -> String {
    m.trim().trim_end_matches('.').trim_end().to_string()
}

/// `404  'pkg@1.0.0' is not in this registry.` → `'pkg@1.0.0' is not in this registry`.
fn not_in_registry(m: &str) -> Option<String> {
    let end = m.find("is not in this registry")? + "is not in this registry".len();
    let start = m.find('\'').filter(|s| *s < end).unwrap_or_else(|| m.find(char::is_whitespace).unwrap_or(0));
    Some(m[start..end].trim().to_string())
}

/// Where generated Dockerfiles put Node projects (`WORKDIR /app`).
const PROJECT_DIR: &str = "/app/";

/// Output lines that never explain a failure on their own (npm notices and
/// warnings, npm error metadata, pointers to log files).
pub(crate) fn is_noise(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    if lower.starts_with("npm notice") || lower.starts_with("npm warn") {
        return true;
    }
    if let Some(m) = npm_messages(&[line]).first() {
        return npm_noise(m) || m.to_ascii_lowercase().starts_with("lifecycle script");
    }
    lower.contains("a complete log of this run can be found")
        || lower.starts_with("info visit https://yarnpkg.com")
        || lower.starts_with("[notice]")
}

/// Lines of an npm error block that explain nothing on their own.
fn npm_noise(m: &str) -> bool {
    let lower = m.to_ascii_lowercase();
    m.len() < 8
        || ["code ", "errno ", "syscall ", "path ", "signal ", "command ", "location ", "workspace "]
            .iter()
            .any(|p| lower.starts_with(p))
        || lower.contains("a complete log of this run")
        || lower.starts_with("for a full report see")
        || lower.starts_with("log files were not written")
        || lower.starts_with("you can rerun the command")
        || m.starts_with('/')
}

fn npm(output: &[&str]) -> Option<String> {
    let msgs = npm_messages(output);
    if msgs.is_empty() {
        return None;
    }
    let field = |name: &str| {
        msgs.iter()
            .rev()
            .find_map(|m| m.strip_prefix(name).and_then(|r| r.strip_prefix(' ')).map(str::trim))
            .filter(|v| !v.is_empty())
    };
    let code = field("code");
    let with_code = |msg: String| match code {
        Some(c) if !msg.contains(c) => format!("{msg} (npm {c})"),
        _ => msg,
    };
    let texts: Vec<&str> = msgs.iter().map(|m| strip_tag(m)).collect();

    // E404: the package (and version) that does not exist.
    if let Some(m) = msgs.iter().find_map(|m| not_in_registry(m)) {
        return Some(with_code(m));
    }
    // ETARGET: the version range nothing satisfies.
    if let Some(m) = texts.iter().find(|m| m.starts_with("No matching version found for")) {
        return Some(with_code(trim_period(m)));
    }
    // ERESOLVE: the conflicting (peer) dependency.
    if let Some(title) = texts.iter().find(|m| m.starts_with("ERESOLVE")) {
        let detail =
            texts.iter().skip_while(|m| !m.starts_with("Could not resolve dependency")).skip(1).find(|m| !m.is_empty());
        let mut s = trim_period(title);
        if let Some(d) = detail {
            s.push_str(&format!(": {d}"));
        }
        if let Some(found) = texts.iter().find_map(|m| m.strip_prefix("Found: ")) {
            s.push_str(&format!(" (found {})", found.trim()));
        }
        s.push_str("; fix the conflict, or set NPM_CONFIG_LEGACY_PEER_DEPS=true to install anyway");
        return Some(s);
    }
    // ENOTFOUND, ECONNREFUSED, ETIMEDOUT...: the request that failed.
    if let Some(m) = texts.iter().find(|m| m.starts_with("request to ") && m.contains(" failed")) {
        return Some(with_code(trim_period(m)));
    }
    // `npm ci` with a stale lock file.
    if texts.iter().any(|m| m.contains("can only install packages when your package.json and package-lock.json")) {
        let details: Vec<String> = texts
            .iter()
            .filter(|m| m.starts_with("Missing: ") || m.starts_with("Invalid: "))
            .take(3)
            .map(|m| trim_period(m))
            .collect();
        let mut s = "package.json and package-lock.json are out of sync".to_string();
        if !details.is_empty() {
            s.push_str(&format!(" ({})", details.join("; ")));
        }
        s.push_str(": run `npm install` and commit the updated package-lock.json");
        return Some(s);
    }
    let code = code?;
    // A numeric code is a failed script (`code 1`, `command sh -c …`). A
    // dependency's install script is worth naming; the project's own
    // scripts are better explained by their own output.
    if code.chars().all(|c| c.is_ascii_digit()) {
        let command = msgs.iter().rev().find_map(|m| m.strip_prefix("command sh -c ")).map(str::trim)?;
        let dependency =
            field("path").and_then(|p| p.strip_prefix(PROJECT_DIR)).filter(|p| p.contains("node_modules"))?;
        return Some(format!("install script `{command}` of {dependency} failed with exit code {code}"));
    }
    // Anything else: npm states the problem first, then explains.
    texts.iter().find(|m| !npm_noise(m)).map(|m| with_code(trim_period(m)))
}

// ---------------------------------------------------------------------------
// pnpm

fn pnpm(output: &[&str]) -> Option<String> {
    let (i, code, msg) = output.iter().enumerate().rev().find_map(|(i, l)| {
        let l = l.trim();
        let rest = l.strip_prefix("ERR_PNPM_")?;
        let (tail, msg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        Some((i, format!("ERR_PNPM_{tail}"), msg.trim()))
    })?;
    // FETCH_404 explains itself a few lines later.
    let detail = output[i + 1..].iter().map(|l| l.trim()).find(|l| l.contains("is not in the npm registry"));
    let msg = trim_period(detail.unwrap_or(msg));
    Some(if msg.is_empty() { code } else { format!("{msg} ({code})") })
}

// ---------------------------------------------------------------------------
// yarn

/// Yarn 2+ (`➤ YN0035: │ pkg@npm:1.0.0: Package not found`): the first
/// message whose code is not informational.
fn yarn_berry(output: &[&str]) -> Option<String> {
    // Progress, build notices and peer-dependency warnings.
    const NOT_ERRORS: &[&str] =
        &["YN0000", "YN0002", "YN0007", "YN0008", "YN0013", "YN0032", "YN0060", "YN0061", "YN0086"];
    output.iter().find_map(|l| {
        let l = l.trim().trim_start_matches('➤').trim();
        let (code, rest) = l.split_once(':')?;
        let is_code = code.len() == 6 && code.starts_with("YN") && code[2..].chars().all(|c| c.is_ascii_digit());
        if !is_code || NOT_ERRORS.contains(&code) {
            return None;
        }
        let msg = trim_period(rest.trim().trim_start_matches(['│', '┌', '└', '·']));
        (!msg.is_empty()).then(|| format!("{msg} (yarn {code})"))
    })
}

/// Yarn 1 (`error Couldn't find package "x@1.0.0" required by …`), only in
/// yarn's output (other tools print `error …` lines too).
fn yarn_classic(output: &[&str]) -> Option<String> {
    let is_yarn = output.iter().any(|l| {
        l.contains("yarnpkg.com")
            || ["yarn install v", "yarn run v", "yarn add v", "yarn v"].iter().any(|p| l.starts_with(p))
    });
    if !is_yarn {
        return None;
    }
    // "Command failed with exit code 1." says nothing the step's output
    // does not say better; "…:" lines introduce more output.
    output
        .iter()
        .filter_map(|l| l.trim().strip_prefix("error "))
        .map(str::trim)
        .filter(|m| !m.is_empty() && !m.ends_with(':'))
        .find(|m| !m.starts_with("Command failed with exit code") && !m.starts_with("Found incompatible module"))
        .map(trim_period)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(lines: &[&str]) -> Option<String> {
        summarize(lines)
    }

    #[test]
    fn npm_e404_names_the_missing_package() {
        // Verbatim npm 10 output (the regression: the summary was the
        // advice's last line, "404 tarball, folder, http url, or git url.").
        let out = [
            "npm error code E404",
            "npm error 404 Not Found - GET https://registry.npmjs.org/ferry-this-package-does-not-exist-xyz - Not found",
            "npm error 404",
            "npm error 404  'ferry-this-package-does-not-exist-xyz@1.0.0' is not in this registry.",
            "npm error 404",
            "npm error 404 Note that you can also install from a",
            "npm error 404 tarball, folder, http url, or git url.",
            "npm notice",
            "npm notice New major version of npm available! 10.9.9 -> 12.1.0",
            "npm notice",
            "npm error A complete log of this run can be found in: /root/.npm/_logs/2026-09-26T13_53_54_546Z-debug-0.log",
        ];
        assert_eq!(
            sum(&out).as_deref(),
            Some("'ferry-this-package-does-not-exist-xyz@1.0.0' is not in this registry (npm E404)")
        );
        // Old npm (`npm ERR!`), and a 404 without the registry line.
        let out = [
            "npm ERR! code E404",
            "npm ERR! 404 Not Found - GET https://registry.npmjs.org/@acme%2fprivate - Not found",
            "npm ERR! 404",
            "npm ERR! A complete log of this run can be found in:",
            "npm ERR!     /root/.npm/_logs/x.log",
        ];
        assert_eq!(
            sum(&out).as_deref(),
            Some("404 Not Found - GET https://registry.npmjs.org/@acme%2fprivate - Not found (npm E404)")
        );
    }

    #[test]
    fn npm_eresolve_etarget_and_network() {
        let out = [
            "npm error code ERESOLVE",
            "npm error ERESOLVE unable to resolve dependency tree",
            "npm error",
            "npm error While resolving: app@1.0.0",
            "npm error Found: react@18.3.1",
            "npm error node_modules/react",
            "npm error   react@\"^18.2.0\" from the root project",
            "npm error",
            "npm error Could not resolve dependency:",
            "npm error peer react@\"^16.8.0\" from react-foo@1.0.0",
            "npm error node_modules/react-foo",
            "npm error",
            "npm error Fix the upstream dependency conflict, or retry",
            "npm error this command with --force or --legacy-peer-deps",
            "npm error to accept an incorrect (and potentially broken) dependency resolution.",
            "npm error A complete log of this run can be found in: /root/.npm/_logs/x.log",
        ];
        assert_eq!(
            sum(&out).as_deref(),
            Some(
                "ERESOLVE unable to resolve dependency tree: peer react@\"^16.8.0\" from react-foo@1.0.0 \
                 (found react@18.3.1); fix the conflict, or set NPM_CONFIG_LEGACY_PEER_DEPS=true to install anyway"
            )
        );

        let out = [
            "npm error code ETARGET",
            "npm error notarget No matching version found for react@^99.0.0.",
            "npm error notarget In most cases you or one of your dependencies are requesting",
            "npm error notarget a package version that doesn't exist.",
        ];
        assert_eq!(sum(&out).as_deref(), Some("No matching version found for react@^99.0.0 (npm ETARGET)"));

        let out = [
            "npm error code ENOTFOUND",
            "npm error syscall getaddrinfo",
            "npm error errno ENOTFOUND",
            "npm error network request to https://registry.npmjs.org/left-pad failed, reason: getaddrinfo ENOTFOUND registry.npmjs.org",
            "npm error network This is a problem related to network connectivity.",
            "npm error network In most cases you are behind a proxy or have bad network settings.",
            "npm error network",
            "npm error network If you are behind a proxy, please make sure that the",
            "npm error network 'proxy' config is set properly.  See: 'npm help config'",
        ];
        assert_eq!(
            sum(&out).as_deref(),
            Some(
                "request to https://registry.npmjs.org/left-pad failed, reason: getaddrinfo ENOTFOUND registry.npmjs.org"
            )
        );
    }

    #[test]
    fn npm_ci_lockfile_scripts_and_other_codes() {
        let out = [
            "npm error code EUSAGE",
            "npm error",
            "npm error `npm ci` can only install packages when your package.json and package-lock.json or npm-shrinkwrap.json are in sync. Please update your lock file with `npm install` before continuing.",
            "npm error",
            "npm error Missing: left-pad@1.3.0 from lock file",
            "npm error",
            "npm error Clean install a project",
            "npm error",
            "npm error Usage:",
            "npm error npm ci",
            "npm error Run \"npm help ci\" for more info",
        ];
        assert_eq!(
            sum(&out).as_deref(),
            Some(
                "package.json and package-lock.json are out of sync (Missing: left-pad@1.3.0 from lock file): \
                 run `npm install` and commit the updated package-lock.json"
            )
        );

        let out = [
            "npm error code 1",
            "npm error path /app/node_modules/bcrypt",
            "npm error command failed",
            "npm error command sh -c node-pre-gyp install --fallback-to-build",
            "npm error gyp ERR! not ok",
        ];
        assert_eq!(
            sum(&out).as_deref(),
            Some(
                "install script `node-pre-gyp install --fallback-to-build` of node_modules/bcrypt failed with exit code 1"
            )
        );
        // The project's own script: its output explains, not npm.
        let out = [
            "npm error Lifecycle script `build` failed with error:",
            "npm error code 2",
            "npm error path /app",
            "npm error command failed",
            "npm error command sh -c tsc",
        ];
        assert_eq!(sum(&out), None);
        assert!(out.iter().all(|l| is_noise(l)), "{out:?}");
        assert!(is_noise("npm notice New major version of npm available!"));
        assert!(!is_noise("src/main.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'."));

        let out = [
            "npm error code EJSONPARSE",
            "npm error path /app/package.json",
            "npm error JSON.parse Unexpected token \"}\" (0x7D) in JSON at position 42 while parsing near \"...\"",
            "npm error JSON.parse Failed to parse JSON data.",
            "npm error JSON.parse Note: package.json must be actual JSON, not just JavaScript.",
        ];
        assert_eq!(
            sum(&out).as_deref(),
            Some(
                "JSON.parse Unexpected token \"}\" (0x7D) in JSON at position 42 while parsing near \"...\" (npm EJSONPARSE)"
            )
        );
        // Only a log pointer: nothing to say.
        assert_eq!(sum(&["npm error A complete log of this run can be found in: /root/.npm/_logs/x.log"]), None);
        assert_eq!(sum(&["npm warn deprecated inflight@1.0.6", "vite v5 building for production..."]), None);
    }

    #[test]
    fn pnpm_errors() {
        let out = [
            " ERR_PNPM_FETCH_404  GET https://registry.npmjs.org/ferry-nope: Not Found - 404",
            "",
            "This error happened while installing a direct dependency of /app",
            "",
            "ferry-nope is not in the npm registry, or you have no permission to fetch it.",
            "",
            "No authorization header was set for the request.",
        ];
        assert_eq!(
            sum(&out).as_deref(),
            Some("ferry-nope is not in the npm registry, or you have no permission to fetch it (ERR_PNPM_FETCH_404)")
        );
        let out = [
            " ERR_PNPM_OUTDATED_LOCKFILE  Cannot install with \"frozen-lockfile\" because pnpm-lock.yaml is not up to date with <ROOT>/package.json",
            "",
            "Note that in CI environments this setting is true by default.",
        ];
        assert_eq!(
            sum(&out).as_deref(),
            Some(
                "Cannot install with \"frozen-lockfile\" because pnpm-lock.yaml is not up to date with \
                 <ROOT>/package.json (ERR_PNPM_OUTDATED_LOCKFILE)"
            )
        );
        // A failed script: pnpm's own line is generic, the step output explains.
        assert_eq!(sum(&[" ELIFECYCLE  Command failed with exit code 1."]), None);
    }

    #[test]
    fn yarn_errors() {
        let out = [
            "yarn install v1.22.22",
            "[1/4] Resolving packages...",
            "error Couldn't find package \"ferry-nope@1.0.0\" required by \"app@1.0.0\" on the \"npm\" registry.",
            "info Visit https://yarnpkg.com/en/docs/cli/install for documentation about this command.",
        ];
        assert_eq!(
            sum(&out).as_deref(),
            Some("Couldn't find package \"ferry-nope@1.0.0\" required by \"app@1.0.0\" on the \"npm\" registry")
        );
        let out = [
            "yarn install v1.22.22",
            "error Your lockfile needs to be updated, but yarn was run with `--frozen-lockfile`.",
            "info Visit https://yarnpkg.com/en/docs/cli/install for documentation about this command.",
        ];
        assert_eq!(
            sum(&out).as_deref(),
            Some("Your lockfile needs to be updated, but yarn was run with `--frozen-lockfile`")
        );
        // A failed build under `yarn build`: nothing yarn-specific to add.
        let out = [
            "yarn run v1.22.22",
            "$ vite build",
            "error during build:",
            "error Command failed with exit code 1.",
            "info Visit https://yarnpkg.com/en/docs/cli/run for documentation about this command.",
        ];
        assert_eq!(sum(&out), None);
        // `error …` lines of other tools are not yarn's.
        assert_eq!(sum(&["error Something broke in my build script"]), None);

        let out = [
            "➤ YN0000: ┌ Resolution step",
            "➤ YN0002: │ app@workspace:. doesn't provide react (p1a2b3), requested by react-foo",
            "➤ YN0035: │ ferry-nope@npm:^1.0.0: Package not found",
            "➤ YN0000: └ Completed in 0s 245ms",
            "➤ YN0000: · Failed with errors in 0s 250ms",
        ];
        assert_eq!(sum(&out).as_deref(), Some("ferry-nope@npm:^1.0.0: Package not found (yarn YN0035)"));
        let out = [
            "➤ YN0000: ┌ Resolution step",
            "➤ YN0028: │ The lockfile would have been modified by this install, which is explicitly forbidden.",
        ];
        assert_eq!(
            sum(&out).as_deref(),
            Some("The lockfile would have been modified by this install, which is explicitly forbidden (yarn YN0028)")
        );
    }
}
