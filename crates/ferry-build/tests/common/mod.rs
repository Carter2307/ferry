//! Shared helpers for ferry-build integration tests.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ferry_build::{BuildRequest, BuildSource};
use ferry_core::{LogLine, Runtime, ServiceType};

/// `examples/<name>` of the workspace.
pub fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}

pub fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).unwrap();
        }
    }
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=Ferry Test", "-c", "user.email=test@ferry.invalid", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Copy an example into `<parent>/<name>`, `git init` + commit it.
/// Returns (repo dir, HEAD sha).
pub fn example_repo(parent: &Path, name: &str) -> (PathBuf, String) {
    let dir = parent.join(name);
    copy_dir(&example(name), &dir);
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", &format!("initial {name}")]);
    let sha = git(&dir, &["rev-parse", "HEAD"]);
    (dir, sha)
}

/// Commit every change in `dir`, returning the new sha.
pub fn commit_all(dir: &Path, msg: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

/// `.tar.gz` of `dir`'s contents, optionally under a top-level directory.
pub fn tar_gz(dir: &Path, top: Option<&str>, out: &Path) {
    let f = fs::File::create(out).unwrap();
    let enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
    let mut b = tar::Builder::new(enc);
    b.append_dir_all(top.unwrap_or("."), dir).unwrap();
    b.into_inner().unwrap().finish().unwrap();
}

pub fn request(
    service_id: &str,
    deploy_id: &str,
    name: &str,
    service_type: ServiceType,
    source: BuildSource,
    image_tag: &str,
) -> BuildRequest {
    BuildRequest {
        service_id: service_id.to_string(),
        deploy_id: deploy_id.to_string(),
        service_name: name.to_string(),
        service_type,
        source,
        runtime: Runtime::Auto,
        root_dir: None,
        dockerfile_path: None,
        build_command: None,
        start_command: None,
        publish_dir: None,
        image_tag: image_tag.to_string(),
        build_args: Vec::new(),
        labels: BTreeMap::new(),
        clear_cache: false,
    }
}

pub fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<LogLine>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(l) = rx.try_recv() {
        out.push(l.line);
    }
    out
}
