//! `ferry up` packaging: a `.tar.gz` of a local directory that respects
//! `.gitignore` (even outside a git repository), `.ignore` and a custom
//! `.ferryignore` (highest precedence, so `!pattern` there can re-include a
//! git-ignored path). Hidden files are included (`.env.example`, `.npmrc`, …);
//! `.git`, `node_modules`, `target`, `.venv` and `__pycache__` are always
//! skipped. Blocking: call from `spawn_blocking`.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use ignore::{DirEntry, WalkBuilder};
use tempfile::NamedTempFile;

/// Directories never uploaded, at any depth.
pub const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".venv", "__pycache__"];

/// Per-directory ignore file specific to Ferry.
pub const IGNORE_FILE: &str = ".ferryignore";

/// Set to stop an in-progress [`pack_dir`] (e.g. the user pressed Ctrl-C);
/// the partial temp archive is then deleted.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Relaxed) { Err(anyhow!("packing canceled")) } else { Ok(()) }
    }
}

/// Cancels when dropped: hold it in the async task awaiting the packing.
#[derive(Debug)]
pub struct CancelOnDrop(pub Cancel);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// A packed directory, kept in a temp file deleted on drop.
#[derive(Debug)]
pub struct Packed {
    pub file: NamedTempFile,
    /// Regular files and symlinks in the archive.
    pub files: usize,
    /// Compressed archive size in bytes.
    pub size: u64,
}

impl Packed {
    pub fn path(&self) -> &Path {
        self.file.path()
    }
}

/// What kind of entry the walk produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
}

/// One path to archive: relative name (with `/` separators) and absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub rel: String,
    pub abs: PathBuf,
    pub kind: EntryKind,
}

fn skipped(entry: &DirEntry) -> bool {
    let name = entry.file_name();
    if name == ".git" {
        return true; // directory, or a file in worktrees/submodules
    }
    let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
    is_dir && name.to_str().is_some_and(|n| SKIP_DIRS.contains(&n))
}

/// List everything that would be uploaded, sorted by path within each
/// directory. Sockets, FIFOs and devices are skipped.
pub fn collect(dir: &Path, cancel: &Cancel) -> Result<Vec<Entry>> {
    let meta = std::fs::metadata(dir).with_context(|| format!("reading {}", dir.display()))?;
    if !meta.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    let mut builder = WalkBuilder::new(dir);
    builder
        .hidden(false)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .require_git(false)
        .follow_links(false)
        .add_custom_ignore_filename(IGNORE_FILE)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| e.depth() == 0 || !skipped(e));

    let mut out = Vec::new();
    for result in builder.build() {
        cancel.check()?;
        let entry = result.map_err(|e| anyhow!("walking {}: {e}", dir.display()))?;
        if entry.depth() == 0 {
            continue;
        }
        let Some(ft) = entry.file_type() else { continue };
        let kind = if ft.is_dir() {
            EntryKind::Dir
        } else if ft.is_file() {
            EntryKind::File
        } else if ft.is_symlink() {
            EntryKind::Symlink
        } else {
            continue;
        };
        let rel_path = entry
            .path()
            .strip_prefix(dir)
            .with_context(|| format!("{} is outside {}", entry.path().display(), dir.display()))?;
        let rel = rel_path
            .components()
            .map(|c| c.as_os_str().to_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| anyhow!("cannot upload {}: file name is not valid UTF-8", entry.path().display()))?
            .join("/");
        out.push(Entry { rel, abs: entry.path().to_path_buf(), kind });
    }
    Ok(out)
}

/// Pack `dir` into a gzip-compressed tarball in a temporary file.
pub fn pack_dir(dir: &Path, cancel: &Cancel) -> Result<Packed> {
    let entries = collect(dir, cancel)?;
    let tmp = tempfile::Builder::new()
        .prefix("ferry-up-")
        .suffix(".tar.gz")
        .tempfile()
        .context("creating a temporary archive file")?;
    let handle = tmp.as_file().try_clone().context("opening the temporary archive file")?;
    let gz = GzEncoder::new(BufWriter::new(handle), Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.follow_symlinks(false);

    let mut files = 0usize;
    for e in &entries {
        cancel.check()?;
        let res = match e.kind {
            EntryKind::Dir => tar.append_dir(&e.rel, &e.abs),
            EntryKind::File | EntryKind::Symlink => {
                files += 1;
                tar.append_path_with_name(&e.abs, &e.rel)
            }
        };
        res.with_context(|| format!("adding {} to the archive", e.abs.display()))?;
    }
    let gz = tar.into_inner().context("finishing the tar archive")?;
    let mut writer = gz.finish().context("compressing the archive")?;
    writer.flush().context("writing the archive")?;
    let file = writer.into_inner().map_err(|e| anyhow!("writing the archive: {}", e.error()))?;
    file.sync_all().context("writing the archive")?;
    let size = tmp.as_file().metadata().context("reading the archive size")?.len();
    Ok(Packed { file: tmp, files, size })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use std::io::Read;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(r, "app.py", "print('hi')");
        write(r, "requirements.txt", "flask\n");
        write(r, ".env.example", "PORT=8000\n");
        write(r, ".gitignore", "*.log\nbuild/\n.env\n");
        write(r, ".ferryignore", "secret.txt\n!keep.log\n");
        write(r, ".env", "SECRET=1\n");
        write(r, "debug.log", "noise");
        write(r, "keep.log", "wanted despite .gitignore");
        write(r, "build/out.js", "x");
        write(r, "secret.txt", "s");
        write(r, "node_modules/left-pad/index.js", "x");
        write(r, "web/node_modules/react/index.js", "x");
        write(r, ".git/config", "[core]");
        write(r, "target/debug/app", "bin");
        write(r, ".venv/bin/python", "py");
        write(r, "__pycache__/a.pyc", "c");
        write(r, "pkg/__pycache__/b.pyc", "c");
        write(r, "pkg/mod.py", "x = 1");
        write(r, "pkg/.hidden/conf", "hidden dir content");
        write(r, "docs/target", "a file named target is kept");
        fs::create_dir_all(r.join("static/empty")).unwrap();
        dir
    }

    fn tar_names(path: &Path) -> BTreeSet<String> {
        let mut bytes = Vec::new();
        fs::File::open(path).unwrap().read_to_end(&mut bytes).unwrap();
        let gz = flate2::read::GzDecoder::new(&bytes[..]);
        let mut ar = tar::Archive::new(gz);
        ar.entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().trim_end_matches('/').to_string())
            .collect()
    }

    #[test]
    fn collect_respects_ignore_rules_and_skip_dirs() {
        let dir = fixture();
        let entries = collect(dir.path(), &Cancel::default()).unwrap();
        let files: BTreeSet<&str> =
            entries.iter().filter(|e| e.kind == EntryKind::File).map(|e| e.rel.as_str()).collect();
        let expected: BTreeSet<&str> = [
            ".env.example",
            ".ferryignore",
            ".gitignore",
            "app.py",
            "docs/target",
            "keep.log",
            "pkg/.hidden/conf",
            "pkg/mod.py",
            "requirements.txt",
        ]
        .into_iter()
        .collect();
        assert_eq!(files, expected);
        let dirs: BTreeSet<&str> =
            entries.iter().filter(|e| e.kind == EntryKind::Dir).map(|e| e.rel.as_str()).collect();
        assert!(dirs.contains("static/empty"), "{dirs:?}");
        for skipped in ["build", "node_modules", "web/node_modules", ".git", "target", ".venv", "__pycache__"] {
            assert!(!dirs.contains(skipped), "{skipped} should be skipped: {dirs:?}");
        }
    }

    #[test]
    fn pack_dir_writes_a_valid_tarball() {
        let dir = fixture();
        let packed = pack_dir(dir.path(), &Cancel::default()).unwrap();
        assert_eq!(packed.files, 9);
        assert!(packed.size > 0);
        assert_eq!(fs::metadata(packed.path()).unwrap().len(), packed.size);
        let names = tar_names(packed.path());
        assert!(names.contains("app.py"));
        assert!(names.contains(".env.example"));
        assert!(names.contains("pkg/mod.py"));
        assert!(names.contains("static/empty"));
        assert!(!names.contains(".env"));
        assert!(!names.contains("secret.txt"));
        assert!(!names.iter().any(|n| n.starts_with(".git/") || n.contains("node_modules")));
        // Content survives the roundtrip.
        let mut bytes = Vec::new();
        fs::File::open(packed.path()).unwrap().read_to_end(&mut bytes).unwrap();
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
        let mut found = false;
        for e in ar.entries().unwrap() {
            let mut e = e.unwrap();
            if e.path().unwrap().to_string_lossy() == "app.py" {
                let mut s = String::new();
                e.read_to_string(&mut s).unwrap();
                assert_eq!(s, "print('hi')");
                found = true;
            }
        }
        assert!(found);
        // The temp file is removed on drop.
        let path = packed.path().to_path_buf();
        drop(packed);
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_stored_as_links() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "real.txt", "x");
        std::os::unix::fs::symlink("real.txt", dir.path().join("link.txt")).unwrap();
        let packed = pack_dir(dir.path(), &Cancel::default()).unwrap();
        assert_eq!(packed.files, 2);
        let bytes = fs::read(packed.path()).unwrap();
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
        let link = ar
            .entries()
            .unwrap()
            .map(Result::unwrap)
            .find(|e| e.path().unwrap().to_string_lossy() == "link.txt")
            .unwrap();
        assert!(link.header().entry_type().is_symlink());
        assert_eq!(link.link_name().unwrap().unwrap().to_string_lossy(), "real.txt");
    }

    #[test]
    fn parent_ignore_rules_apply_but_never_exclude_the_chosen_root() {
        // A static site's build output is usually git-ignored at the repo root.
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), ".gitignore", "dist/\n*.log\nbuild\n");
        write(repo.path(), "dist/index.html", "<h1>hi</h1>");
        write(repo.path(), "dist/assets/app.js", "x");
        write(repo.path(), "dist/debug.log", "noise");
        write(repo.path(), "dist/build/out.txt", "ignored by the parent rule 'build'");
        let entries = collect(&repo.path().join("dist"), &Cancel::default()).unwrap();
        let files: BTreeSet<&str> =
            entries.iter().filter(|e| e.kind == EntryKind::File).map(|e| e.rel.as_str()).collect();
        assert_eq!(files, BTreeSet::from(["assets/app.js", "index.html"]));
    }

    #[test]
    fn not_a_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "f", "x");
        assert!(collect(&dir.path().join("f"), &Cancel::default()).is_err());
        assert!(collect(&dir.path().join("missing"), &Cancel::default()).is_err());
    }

    #[test]
    fn canceled_packing_stops_and_leaves_no_file() {
        let dir = fixture();
        let cancel = Cancel::default();
        drop(CancelOnDrop(cancel.clone()));
        let err = pack_dir(dir.path(), &cancel).unwrap_err();
        assert!(err.to_string().contains("canceled"));
    }
}
