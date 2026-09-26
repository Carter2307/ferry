//! Safe tar extraction (uploaded `.tar.gz` sources and `git archive` output).
//!
//! Guarantees: nothing is written outside the destination directory, entries
//! with absolute paths or `..` components are rejected, and no symlink left
//! in the tree resolves outside of it. Device files and FIFOs are skipped.
//! Directory permissions are not restored (the tree must stay writable so it
//! can be cleaned up); file modes are kept minus group/other write.

use std::cell::RefCell;
use std::fs;
use std::io::{self, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use ferry_core::CancellationToken;

/// Limits protecting the disk (and memory) against decompression bombs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    /// Total size of the regular files once extracted.
    pub max_bytes: u64,
    /// Entries of any kind (files, directories, links).
    pub max_entries: u64,
    /// What is extracted, for messages ("the uploaded archive").
    pub subject: &'static str,
    /// Appended to limit errors: how to get under the limit.
    pub advice: &'static str,
}

impl Limits {
    /// `git archive` exports: generous, they only protect the disk.
    pub(crate) const GIT: Limits =
        Limits { max_bytes: 16 * 1024 * 1024 * 1024, max_entries: 2_000_000, subject: "the source tree", advice: "" };

    /// Uploaded sources (`ferry up`) with the given caps.
    pub(crate) fn upload(max_bytes: u64, max_entries: u64) -> Limits {
        Limits {
            max_bytes,
            max_entries,
            subject: "the uploaded archive",
            advice: ": leave dependencies, build outputs and large data out of it (.gitignore / .ferryignore)",
        }
    }

    fn too_big(&self) -> ExtractError {
        ExtractError::Invalid(format!(
            "{} expands to more than {} (the limit for extracted sources){}",
            self.subject,
            crate::human_bytes(self.max_bytes),
            self.advice
        ))
    }

    fn too_many(&self) -> ExtractError {
        ExtractError::Invalid(format!(
            "{} has more than {} entries (the limit for extracted sources){}",
            self.subject, self.max_entries, self.advice
        ))
    }

    /// Everything the decompressed stream may yield: the files plus tar
    /// metadata (headers, long names, padding) for every allowed entry.
    fn stream_budget(&self) -> u64 {
        self.max_bytes.saturating_add(self.max_entries.saturating_mul(2048)).saturating_add(METADATA_BUDGET)
    }
}

/// Bytes the tar reader may consume between two entries (headers, PAX / GNU
/// long-name records, which the tar crate buffers in memory, data of
/// skipped entries). Real archives need a few KiB.
const METADATA_BUDGET: u64 = 16 * 1024 * 1024;
/// Bytes read after the end-of-archive marker (a `git archive` pipe is
/// drained so git exits cleanly; its padding is at most 10 KiB).
const DRAIN_BUDGET: u64 = 1024 * 1024;

/// Read budget shared by [`Capped`] and the extraction loop.
#[derive(Debug)]
struct Budget {
    total: u64,
    /// Extra cap for the current phase (`None`: only `total`).
    phase: Option<u64>,
    /// Which cap refused a read, if any.
    exhausted: Option<Cap>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cap {
    Total,
    Phase,
}

/// A reader that refuses to yield more than its [`Budget`], so a
/// decompression bomb cannot burn CPU or memory where the per-entry checks
/// do not look.
struct Capped<R> {
    inner: R,
    budget: Rc<RefCell<Budget>>,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let (allowed, cap) = {
            let b = self.budget.borrow();
            match b.phase {
                Some(p) if p < b.total => (p, Cap::Phase),
                _ => (b.total, Cap::Total),
            }
        };
        if buf.is_empty() {
            return Ok(0);
        }
        if allowed == 0 {
            self.budget.borrow_mut().exhausted = Some(cap);
            return Err(io::Error::other("extraction size limit reached"));
        }
        let want = buf.len().min(usize::try_from(allowed).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..want])?;
        let mut b = self.budget.borrow_mut();
        b.total -= n as u64;
        if let Some(p) = b.phase.as_mut() {
            *p -= n as u64;
        }
        Ok(n)
    }
}

/// What to do with a symlink (or hard link) pointing outside the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkPolicy {
    /// Fail the extraction (untrusted uploads).
    Reject,
    /// Drop the link and report it (git checkouts, where Docker would ignore
    /// such links anyway).
    Skip,
}

#[derive(Debug)]
pub(crate) enum ExtractError {
    Canceled,
    /// The archive is malformed or violates a safety rule.
    Invalid(String),
    Io(io::Error),
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Canceled => f.write_str("canceled"),
            ExtractError::Invalid(m) => f.write_str(m),
            ExtractError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for ExtractError {
    fn from(e: io::Error) -> Self {
        ExtractError::Io(e)
    }
}

/// Outcome of a successful extraction.
#[derive(Debug, Default)]
pub(crate) struct ExtractReport {
    pub files: u64,
    pub bytes: u64,
    /// Human-readable notes about skipped entries.
    pub skipped: Vec<String>,
}

/// Extract a `.tar.gz` (or plain `.tar`) file into `dest` (which must exist).
pub(crate) fn extract_archive_file(
    archive: &Path,
    dest: &Path,
    limits: Limits,
    cancel: &CancellationToken,
) -> Result<ExtractReport, ExtractError> {
    let file = fs::File::open(archive).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            ExtractError::Invalid(format!("source archive {} not found", archive.display()))
        } else {
            ExtractError::Io(e)
        }
    })?;
    let mut reader = BufReader::new(file);
    let mut magic = [0u8; 2];
    let n = read_up_to(&mut reader, &mut magic)?;
    if n == 0 {
        return Err(ExtractError::Invalid("source archive is empty".into()));
    }
    // Re-chain the sniffed bytes in front of the rest of the stream.
    let chained = io::Cursor::new(magic[..n].to_vec()).chain(reader);
    if n == 2 && magic == [0x1f, 0x8b] {
        extract_tar(flate2::read::MultiGzDecoder::new(chained), dest, LinkPolicy::Reject, limits, cancel)
    } else {
        extract_tar(chained, dest, LinkPolicy::Reject, limits, cancel)
    }
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Extract a tar stream into `dest` (which must exist), within `limits`:
/// sizes are checked from each entry's header before anything is written,
/// and the stream itself is capped. The stream is consumed up to a little
/// past the end of the archive (so a writer on the other end of a pipe
/// never blocks).
pub(crate) fn extract_tar<R: Read>(
    reader: R,
    dest: &Path,
    links: LinkPolicy,
    limits: Limits,
    cancel: &CancellationToken,
) -> Result<ExtractReport, ExtractError> {
    let root = fs::canonicalize(dest)?;
    let budget = Rc::new(RefCell::new(Budget { total: limits.stream_budget(), phase: None, exhausted: None }));
    let set_phase = |phase: Option<u64>| budget.borrow_mut().phase = phase;
    // A refused read surfaces as some I/O error of the tar crate: report the
    // limit instead.
    let or_limit = |e: ExtractError| match budget.borrow().exhausted {
        Some(Cap::Total) => limits.too_big(),
        Some(Cap::Phase) => ExtractError::Invalid(format!(
            "{} has more than {} of tar metadata between two entries (not a usable source archive)",
            limits.subject,
            crate::human_bytes(METADATA_BUDGET)
        )),
        None => e,
    };
    let mut archive = tar::Archive::new(Capped { inner: reader, budget: budget.clone() });
    archive.set_preserve_permissions(false);
    archive.set_preserve_ownerships(false);
    archive.set_unpack_xattrs(false);
    // Extraction time as mtime: guarantees rebuilt sources look newer than any
    // cached build output (cargo fingerprints are mtime based).
    archive.set_preserve_mtime(false);
    archive.set_overwrite(true);
    archive.set_mask(0o022);

    let mut report = ExtractReport::default();
    let mut entries_seen: u64 = 0;
    {
        let mut entries = archive.entries().map_err(|e| invalid_archive(&e))?;
        loop {
            if cancel.is_cancelled() {
                return Err(ExtractError::Canceled);
            }
            set_phase(Some(METADATA_BUDGET));
            let Some(entry) = entries.next() else { break };
            set_phase(None);
            let mut entry = entry.map_err(|e| or_limit(invalid_archive(&e)))?;
            let kind = entry.header().entry_type();
            if kind.is_pax_global_extensions()
                || kind.is_pax_local_extensions()
                || kind.is_gnu_longname()
                || kind.is_gnu_longlink()
            {
                continue;
            }
            entries_seen += 1;
            if entries_seen > limits.max_entries {
                return Err(limits.too_many());
            }
            let raw_path = entry.path().map_err(|e| invalid_archive(&e))?.into_owned();
            let rel = sanitize_entry_path(&raw_path)?;
            if rel.as_os_str().is_empty() {
                continue; // the archive root itself ("./")
            }
            let display = rel.display().to_string();

            if kind.is_dir() {
                safe_create_dirs(&root, &rel, &display)?;
                continue;
            }
            if kind.is_symlink() {
                let target = entry
                    .link_name()
                    .map_err(|e| invalid_archive(&e))?
                    .ok_or_else(|| ExtractError::Invalid(format!("symlink '{display}' has no target")))?
                    .into_owned();
                // Judge the target from the link's *real* directory (its parent
                // may itself be reached through a symlink).
                let parent = rel.parent().unwrap_or(Path::new(""));
                let real_parent = safe_create_dirs(&root, parent, &display)?;
                let rel_parent = real_parent.strip_prefix(&root).unwrap_or(Path::new("")).to_path_buf();
                if !lexically_inside(&rel_parent, &target) {
                    match links {
                        LinkPolicy::Reject => {
                            return Err(ExtractError::Invalid(format!(
                                "symlink '{display}' -> '{}' points outside the source tree",
                                target.display()
                            )));
                        }
                        LinkPolicy::Skip => {
                            report.skipped.push(format!(
                                "symlink {display} -> {} (points outside the source tree)",
                                target.display()
                            ));
                            continue;
                        }
                    }
                }
            } else if kind.is_hard_link() {
                let target = entry
                    .link_name()
                    .map_err(|e| invalid_archive(&e))?
                    .ok_or_else(|| ExtractError::Invalid(format!("hard link '{display}' has no target")))?
                    .into_owned();
                if let Err(e) = sanitize_entry_path(&target) {
                    match links {
                        LinkPolicy::Reject => return Err(e),
                        LinkPolicy::Skip => {
                            report.skipped.push(format!("hard link {display} ({e})"));
                            continue;
                        }
                    }
                }
            } else if kind.is_file() || kind.is_contiguous() || kind.is_gnu_sparse() {
                report.files += 1;
                report.bytes = report.bytes.saturating_add(entry.size());
                if report.bytes > limits.max_bytes {
                    return Err(limits.too_big());
                }
            } else {
                report.skipped.push(format!("{display} (unsupported entry type {:?})", kind));
                continue;
            }

            let unpacked = entry.unpack_in(&root).map_err(|e| {
                or_limit(ExtractError::Invalid(format!("cannot extract '{display}': {}", flatten_io_error(&e))))
            })?;
            if !unpacked {
                return Err(ExtractError::Invalid(format!("archive entry '{display}' escapes the source directory")));
            }
        }
    }
    // Drain trailing padding so the producer (e.g. `git archive`) exits
    // cleanly, but never a bomb hidden after the end of the archive.
    set_phase(Some(DRAIN_BUDGET));
    let mut rest = archive.into_inner();
    let _ = io::copy(&mut rest, &mut io::sink());

    verify_links(&root, links, &mut report, cancel)?;
    Ok(report)
}

fn invalid_archive(e: &io::Error) -> ExtractError {
    ExtractError::Invalid(format!("invalid source archive: {}", flatten_io_error(e)))
}

/// `tar` wraps errors in several layers; show the whole chain on one line.
fn flatten_io_error(e: &io::Error) -> String {
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        let m = s.to_string();
        if !msg.contains(&m) {
            msg.push_str(": ");
            msg.push_str(&m);
        }
        src = s.source();
    }
    msg
}

/// Normalize an entry path: reject absolute paths and `..`, drop `.`.
pub(crate) fn sanitize_entry_path(p: &Path) -> Result<PathBuf, ExtractError> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(ExtractError::Invalid(format!(
                    "archive entry '{}' contains '..' (path traversal)",
                    p.display()
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ExtractError::Invalid(format!("archive entry '{}' is an absolute path", p.display())));
            }
        }
    }
    Ok(out)
}

/// Would `target`, resolved relative to directory `base` (relative to the
/// tree root, symlinks not followed), stay inside the tree?
pub(crate) fn lexically_inside(base: &Path, target: &Path) -> bool {
    let mut depth: Vec<&std::ffi::OsStr> = Vec::new();
    for c in base.components() {
        match c {
            Component::Normal(p) => depth.push(p),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth.pop().is_none() {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    for c in target.components() {
        match c {
            Component::Normal(p) => depth.push(p),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth.pop().is_none() {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

/// Create `rel` (relative to the canonical `root`) one component at a time,
/// refusing to follow any existing symlink that leads outside `root`.
/// Returns the canonical path of the directory.
fn safe_create_dirs(root: &Path, rel: &Path, display: &str) -> Result<PathBuf, ExtractError> {
    let mut cur = root.to_path_buf();
    for c in rel.components() {
        let Component::Normal(part) = c else { continue };
        let next = cur.join(part);
        match fs::symlink_metadata(&next) {
            Ok(_) => {
                let canon = fs::canonicalize(&next).map_err(|e| {
                    ExtractError::Invalid(format!("cannot extract '{display}': {}: {e}", next.display()))
                })?;
                if !canon.starts_with(root) {
                    return Err(ExtractError::Invalid(format!(
                        "archive entry '{display}' escapes the source directory"
                    )));
                }
                if !canon.is_dir() {
                    return Err(ExtractError::Invalid(format!(
                        "cannot extract '{display}': a parent path is not a directory"
                    )));
                }
                cur = canon;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&next)?;
                cur = next;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(cur)
}

/// Final safety net: walk the extracted tree and check that every symlink
/// resolves inside it, following chains of links for real. Dangling links are
/// checked lexically from their real parent directory.
fn verify_links(
    root: &Path,
    policy: LinkPolicy,
    report: &mut ExtractReport,
    cancel: &CancellationToken,
) -> Result<(), ExtractError> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.is_cancelled() {
            return Err(ExtractError::Canceled);
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            let path = entry.path();
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_symlink() {
                continue;
            }
            let inside = match fs::canonicalize(&path) {
                Ok(target) => target.starts_with(root),
                Err(_) => {
                    let target = fs::read_link(&path)?;
                    let real_parent = fs::canonicalize(&dir)?;
                    match real_parent.strip_prefix(root) {
                        Ok(rel_parent) => lexically_inside(rel_parent, &target),
                        Err(_) => false,
                    }
                }
            };
            if !inside {
                let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
                match policy {
                    LinkPolicy::Reject => {
                        return Err(ExtractError::Invalid(format!("symlink '{rel}' points outside the source tree")));
                    }
                    LinkPolicy::Skip => {
                        fs::remove_file(&path)?;
                        report.skipped.push(format!("symlink {rel} (points outside the source tree)"));
                    }
                }
            }
        }
    }
    Ok(())
}

/// If `dir` contains exactly one entry and it is a real directory, return it
/// (archives of the form `project/...`).
pub(crate) fn single_top_level_dir(dir: &Path) -> io::Result<Option<PathBuf>> {
    let mut entries = fs::read_dir(dir)?;
    let Some(first) = entries.next().transpose()? else { return Ok(None) };
    if entries.next().is_some() {
        return Ok(None);
    }
    if first.file_type()?.is_dir() { Ok(Some(first.path())) } else { Ok(None) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn header(path: &str, kind: tar::EntryType, size: u64) -> tar::Header {
        let mut h = tar::Header::new_gnu();
        // Write the raw name so tests can produce malicious paths that
        // `Header::set_path` would refuse.
        {
            let name = &mut h.as_old_mut().name;
            name[..path.len()].copy_from_slice(path.as_bytes());
        }
        h.set_entry_type(kind);
        h.set_size(size);
        h.set_mode(if kind == tar::EntryType::Directory { 0o755 } else { 0o644 });
        h.set_cksum();
        h
    }

    fn tar_bytes(entries: &[(&str, tar::EntryType, &[u8], Option<&str>)]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (path, kind, data, link) in entries {
            let mut h = header(path, *kind, data.len() as u64);
            if let Some(l) = link {
                let ln = &mut h.as_old_mut().linkname;
                ln[..l.len()].copy_from_slice(l.as_bytes());
                h.set_cksum();
            }
            b.append(&h, *data).unwrap();
        }
        b.into_inner().unwrap()
    }

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn extract(bytes: Vec<u8>, policy: LinkPolicy) -> (tempfile::TempDir, Result<ExtractReport, ExtractError>) {
        let dir = tempfile::tempdir().unwrap();
        let res = extract_tar(&bytes[..], dir.path(), policy, Limits::GIT, &CancellationToken::new());
        (dir, res)
    }

    use tar::EntryType as E;

    #[test]
    fn extracts_regular_tree() {
        let bytes = tar_bytes(&[
            ("./", E::Directory, b"", None),
            ("app/", E::Directory, b"", None),
            ("app/index.html", E::Regular, b"<h1>hi</h1>", None),
            ("link", E::Symlink, b"", Some("app/index.html")),
        ]);
        let (dir, res) = extract(bytes, LinkPolicy::Reject);
        let report = res.unwrap();
        assert_eq!(report.files, 1);
        assert_eq!(fs::read_to_string(dir.path().join("app/index.html")).unwrap(), "<h1>hi</h1>");
        assert_eq!(fs::read_to_string(dir.path().join("link")).unwrap(), "<h1>hi</h1>");
    }

    #[test]
    fn rejects_parent_traversal() {
        let bytes = tar_bytes(&[("../evil.txt", E::Regular, b"x", None)]);
        let (dir, res) = extract(bytes, LinkPolicy::Reject);
        let err = res.unwrap_err().to_string();
        assert!(err.contains("path traversal"), "{err}");
        assert!(!dir.path().parent().unwrap().join("evil.txt").exists());

        let bytes = tar_bytes(&[("a/../../evil.txt", E::Regular, b"x", None)]);
        assert!(extract(bytes, LinkPolicy::Reject).1.is_err());
    }

    #[test]
    fn rejects_absolute_paths() {
        let bytes = tar_bytes(&[("/tmp/ferry-evil-abs.txt", E::Regular, b"x", None)]);
        let err = extract(bytes, LinkPolicy::Reject).1.unwrap_err().to_string();
        assert!(err.contains("absolute"), "{err}");
    }

    #[test]
    fn rejects_escaping_symlinks() {
        for target in ["/etc/passwd", "../outside", "a/../../outside"] {
            let bytes = tar_bytes(&[("link", E::Symlink, b"", Some(target))]);
            let err = extract(bytes, LinkPolicy::Reject).1.unwrap_err().to_string();
            assert!(err.contains("outside the source tree"), "{target}: {err}");
        }
    }

    #[test]
    fn rejects_writes_through_symlinked_dirs() {
        // `d -> .` is harmless, but a symlink created through a symlinked parent
        // must be judged from its real location.
        let bytes = tar_bytes(&[
            ("sub/", E::Directory, b"", None),
            ("sub/up", E::Symlink, b"", Some("..")),
            ("sub/up/evil", E::Symlink, b"", Some("../x")),
        ]);
        let (_dir, res) = extract(bytes, LinkPolicy::Reject);
        assert!(res.is_err(), "{res:?}");
    }

    #[test]
    fn skip_policy_drops_escaping_links() {
        let bytes = tar_bytes(&[
            ("ok.txt", E::Regular, b"ok", None),
            ("abs", E::Symlink, b"", Some("/etc/passwd")),
            ("hard", E::Link, b"", Some("../x")),
        ]);
        let (dir, res) = extract(bytes, LinkPolicy::Skip);
        let report = res.unwrap();
        assert_eq!(report.skipped.len(), 2, "{:?}", report.skipped);
        assert!(dir.path().join("ok.txt").exists());
        assert!(fs::symlink_metadata(dir.path().join("abs")).is_err());
    }

    #[test]
    fn skips_device_files() {
        let bytes = tar_bytes(&[("fifo", E::Fifo, b"", None), ("a.txt", E::Regular, b"a", None)]);
        let (dir, res) = extract(bytes, LinkPolicy::Reject);
        assert_eq!(res.unwrap().skipped.len(), 1);
        assert!(!dir.path().join("fifo").exists());
    }

    #[test]
    fn honors_cancellation() {
        let bytes = tar_bytes(&[("a.txt", E::Regular, b"a", None)]);
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            extract_tar(&bytes[..], dir.path(), LinkPolicy::Reject, Limits::GIT, &cancel),
            Err(ExtractError::Canceled)
        ));
    }

    #[test]
    fn archive_file_gzip_and_plain() {
        let tar = tar_bytes(&[("proj/", E::Directory, b"", None), ("proj/package.json", E::Regular, b"{}", None)]);
        let src = tempfile::tempdir().unwrap();
        for (name, data) in [("a.tar.gz", gz(&tar)), ("b.tar", tar.clone())] {
            let p = src.path().join(name);
            fs::write(&p, data).unwrap();
            let dest = tempfile::tempdir().unwrap();
            extract_archive_file(&p, dest.path(), Limits::GIT, &CancellationToken::new()).unwrap();
            let top = single_top_level_dir(dest.path()).unwrap().unwrap();
            assert_eq!(top.file_name().unwrap(), "proj");
            assert!(top.join("package.json").exists());
        }
        let missing =
            extract_archive_file(&src.path().join("nope.tar.gz"), src.path(), Limits::GIT, &CancellationToken::new());
        assert!(missing.unwrap_err().to_string().contains("not found"));
        let garbage = src.path().join("garbage.tar.gz");
        fs::write(&garbage, [0x1f, 0x8b, 1, 2, 3]).unwrap();
        let dest = tempfile::tempdir().unwrap();
        assert!(extract_archive_file(&garbage, dest.path(), Limits::GIT, &CancellationToken::new()).is_err());
    }

    fn upload_limits(max_bytes: u64, max_entries: u64) -> Limits {
        Limits::upload(max_bytes, max_entries)
    }

    #[test]
    fn decompression_bombs_are_refused_before_filling_the_disk() {
        const MIB: u64 = 1024 * 1024;
        // Regression: a 3 MB upload expanded to 3 GiB and went live. The
        // header announces the size: refused before a byte is written.
        let mut big = tar::Builder::new(Vec::new());
        big.append(&header("app/zeros.bin", E::Regular, 4 * MIB), io::repeat(0).take(4 * MIB)).unwrap();
        let big = gz(&big.into_inner().unwrap());
        assert!(big.len() < 64 * 1024, "{}", big.len());
        let src = tempfile::tempdir().unwrap();
        let path = src.path().join("bomb.tar.gz");
        fs::write(&path, &big).unwrap();
        let dest = tempfile::tempdir().unwrap();
        let err = extract_archive_file(&path, dest.path(), upload_limits(MIB, 100), &CancellationToken::new())
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "the uploaded archive expands to more than 1.0 MiB (the limit for extracted sources): leave \
             dependencies, build outputs and large data out of it (.gitignore / .ferryignore)"
        );
        assert!(!dest.path().join("app/zeros.bin").exists());
        // Within the limit it extracts.
        let dest = tempfile::tempdir().unwrap();
        let report =
            extract_archive_file(&path, dest.path(), upload_limits(8 * MIB, 100), &CancellationToken::new()).unwrap();
        assert_eq!(report.bytes, 4 * MIB);

        // The default upload limit: a 3 GiB entry is refused from its header
        // alone (the data does not even need to follow).
        let mut h = header("zeros.bin", E::Regular, 0);
        h.set_size(3 * 1024 * MIB);
        h.set_cksum();
        let mut bytes = h.as_bytes().to_vec();
        bytes.extend_from_slice(&[0u8; 4096]);
        let dest = tempfile::tempdir().unwrap();
        let limits = upload_limits(crate::DEFAULT_MAX_UPLOAD_EXTRACTED_BYTES, crate::DEFAULT_MAX_UPLOAD_ENTRIES);
        let err =
            extract_tar(&bytes[..], dest.path(), LinkPolicy::Reject, limits, &CancellationToken::new()).unwrap_err();
        assert!(err.to_string().starts_with("the uploaded archive expands to more than 2.0 GiB"), "{err}");
        assert!(fs::read_dir(dest.path()).unwrap().next().is_none());
    }

    #[test]
    fn entry_count_is_limited() {
        let names: Vec<String> = (0..20).map(|i| format!("f{i}.txt")).collect();
        let entries: Vec<(&str, E, &[u8], Option<&str>)> =
            names.iter().map(|n| (n.as_str(), E::Regular, &b"x"[..], None)).collect();
        let bytes = tar_bytes(&entries);
        let dest = tempfile::tempdir().unwrap();
        let err = extract_tar(
            &bytes[..],
            dest.path(),
            LinkPolicy::Reject,
            upload_limits(1 << 30, 10),
            &CancellationToken::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.starts_with("the uploaded archive has more than 10 entries (the limit for extracted sources)"),
            "{err}"
        );
        let dest = tempfile::tempdir().unwrap();
        let ok = extract_tar(
            &bytes[..],
            dest.path(),
            LinkPolicy::Reject,
            upload_limits(1 << 30, 20),
            &CancellationToken::new(),
        );
        assert_eq!(ok.unwrap().files, 20);
    }

    /// Counts what the extractor pulls from the stream.
    struct Counting<'a> {
        inner: &'a [u8],
        read: std::rc::Rc<std::cell::Cell<u64>>,
    }

    impl Read for Counting<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.read.set(self.read.get() + n as u64);
            Ok(n)
        }
    }

    #[test]
    fn metadata_and_trailing_bombs_are_capped() {
        const MIB: usize = 1024 * 1024;
        // A 20 MiB PAX header (the tar crate would buffer it in memory).
        let mut bytes = Vec::new();
        let pax = vec![b'a'; 20 * MIB];
        let mut h = header("PaxHeaders/x", E::XHeader, pax.len() as u64);
        h.set_cksum();
        bytes.extend_from_slice(h.as_bytes());
        bytes.extend_from_slice(&pax);
        bytes.extend_from_slice(&tar_bytes(&[("a.txt", E::Regular, b"a", None)]));
        let dest = tempfile::tempdir().unwrap();
        let read = std::rc::Rc::new(std::cell::Cell::new(0));
        let reader = Counting { inner: &bytes, read: read.clone() };
        let err = extract_tar(reader, dest.path(), LinkPolicy::Skip, Limits::GIT, &CancellationToken::new())
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "the source tree has more than 16.0 MiB of tar metadata between two entries (not a usable source archive)"
        );
        assert!(read.get() <= (16 * MIB + 64 * 1024) as u64, "{}", read.get());

        // Garbage after the end of the archive is not read to the end.
        let mut bytes = tar_bytes(&[("a.txt", E::Regular, b"a", None)]);
        let archive_len = bytes.len() as u64;
        bytes.extend(std::iter::repeat_n(0u8, 32 * MIB));
        let dest = tempfile::tempdir().unwrap();
        let read = std::rc::Rc::new(std::cell::Cell::new(0));
        let reader = Counting { inner: &bytes, read: read.clone() };
        let report =
            extract_tar(reader, dest.path(), LinkPolicy::Reject, Limits::GIT, &CancellationToken::new()).unwrap();
        assert_eq!(report.files, 1);
        assert!(read.get() <= archive_len + 2 * MIB as u64, "{}", read.get());
    }

    #[test]
    fn single_top_level_dir_rules() {
        let d = tempfile::tempdir().unwrap();
        assert!(single_top_level_dir(d.path()).unwrap().is_none());
        fs::create_dir(d.path().join("app")).unwrap();
        assert!(single_top_level_dir(d.path()).unwrap().is_some());
        fs::write(d.path().join("README"), "x").unwrap();
        assert!(single_top_level_dir(d.path()).unwrap().is_none());
        let f = tempfile::tempdir().unwrap();
        fs::write(f.path().join("only-file"), "x").unwrap();
        assert!(single_top_level_dir(f.path()).unwrap().is_none());
    }

    #[test]
    fn lexical_checks() {
        assert!(lexically_inside(Path::new("a/b"), Path::new("../c")));
        assert!(lexically_inside(Path::new("a"), Path::new("../c")));
        assert!(!lexically_inside(Path::new(""), Path::new("../c")));
        assert!(!lexically_inside(Path::new("a"), Path::new("/c")));
        assert!(lexically_inside(Path::new(""), Path::new("./x/./y")));
    }
}
