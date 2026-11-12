//! [`nul_bytes`]: no tracked `.rs`, `.toml` or `.md` file holds a raw NUL byte
//! . Git takes a file with one for binary
//! and merges it by keeping one side whole: the other side's changes are dropped, with no conflict
//! markers to say so.
//!
//! The files are git's tracked ones (`git ls-files`), so an ignored or untracked file never fails
//! the check; outside a repository (a test's fixture) they are the walked workspace's. A file is
//! read in chunks, so its size doesn't matter, and a tracked file missing from the tree (deleted,
//! not yet committed) is skipped.

use std::io::Read;
use std::path::Path;

use super::Finding;

/// The extensions read: what git merges as text in every repo on Swarm Core.
const EXTENSIONS: &[&str] = &[".rs", ".toml", ".md"];

/// How much of a file is read at a time.
const CHUNK: usize = 64 * 1024;

/// A finding for each of `files` (the workspace's, from [`super::manifest::files`]: git's, where
/// `root` is in a repository) that holds a raw NUL byte, at its first one.
pub(crate) fn nul_bytes(root: &Path, files: &[String]) -> Vec<Finding> {
    files.iter().filter(|f| EXTENSIONS.iter().any(|ext| f.ends_with(ext))).filter_map(|f| first_nul(root, f)).collect()
}

/// The finding for `shown`'s first NUL byte, an unreadable file's, or `None`.
fn first_nul(root: &Path, shown: &str) -> Option<Finding> {
    let unreadable = |e: std::io::Error| Some(Finding::new(shown, None, "unreadable", format!("cannot read it: {e}"), "make the file readable"));
    let mut file = match std::fs::File::open(root.join(shown)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return unreadable(e),
    };
    let (mut buf, mut offset, mut line) = (vec![0u8; CHUNK], 0usize, 1usize);
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => return None,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return unreadable(e),
        };
        let chunk = buf.get(..n).unwrap_or_default();
        if let Some(at) = chunk.iter().position(|b| *b == 0) {
            let before = chunk.get(..at).unwrap_or_default();
            let line = line.saturating_add(before.iter().filter(|b| **b == b'\n').count());
            let problem = format!(
                "a raw NUL byte at byte {}: git takes the file for binary and merges it by keeping one side whole, dropping the other's changes",
                offset.saturating_add(at)
            );
            let fix = "remove it (in a string literal, write the escape `\\0`), or give a binary fixture an extension other than .rs, .toml or .md";
            return Some(Finding::new(shown, Some(line), "nul-byte", problem, fix));
        }
        line = line.saturating_add(chunk.iter().filter(|b| **b == b'\n').count());
        offset = offset.saturating_add(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str, files: &[(&str, &[u8])]) -> std::path::PathBuf {
        let dir = swarm_core_testkit::temp_dir(name);
        for (path, bytes) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        dir
    }

    fn walked(dir: &Path) -> Vec<String> {
        super::super::manifest::files(dir).0
    }

    #[test]
    fn a_nul_byte_is_found_with_its_line_and_offset() {
        let dir = fixture("nul-found", &[("a/src/lib.rs", b"fn f() {}\n// x\0y\n"), ("README.md", b"# fine\n"), ("data.bin", b"\0\0")]);
        let found = nul_bytes(&dir, &walked(&dir));
        assert_eq!(found.len(), 1, "{found:?}");
        let f = &found[0];
        assert_eq!((f.file.as_str(), f.line, f.rule), ("a/src/lib.rs", Some(2), "nul-byte"));
        assert!(f.problem.contains("at byte 14"), "{}", f.problem);
    }

    #[test]
    fn each_extension_is_read_and_others_are_not() {
        let dir = fixture("nul-exts", &[("a.rs", b"\0"), ("Cargo.toml", b"x\0"), ("doc.md", b"\n\n\0"), ("fixture.json", b"\0"), ("b.rs", b"\\0 is fine")]);
        let found: Vec<(String, Option<usize>)> = nul_bytes(&dir, &walked(&dir)).into_iter().map(|f| (f.file, f.line)).collect();
        assert_eq!(found, [("Cargo.toml".to_owned(), Some(1)), ("a.rs".to_owned(), Some(1)), ("doc.md".to_owned(), Some(3))]);
    }

    #[test]
    fn a_nul_past_the_first_chunk_is_found() {
        let mut big = vec![b'a'; CHUNK + 10];
        big[CHUNK - 1] = b'\n';
        big[CHUNK + 5] = 0;
        let dir = fixture("nul-chunk", &[("big.md", &big)]);
        let found = nul_bytes(&dir, &walked(&dir));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, Some(2));
        assert!(found[0].problem.contains(&format!("at byte {}", CHUNK + 5)), "{}", found[0].problem);
    }

    #[test]
    fn in_a_repository_only_tracked_files_are_read() {
        let dir = fixture("nul-tracked", &[("tracked.rs", b"fn f() {}\n"), ("ignored.md", b"\0"), (".gitignore", b"ignored.md\n")]);
        swarm_core_testkit::git::git(&dir, &["init", "--quiet"]);
        swarm_core_testkit::git::git(&dir, &["add", "tracked.rs", ".gitignore"]);
        assert!(nul_bytes(&dir, &walked(&dir)).is_empty());
        // Tracked, it is found; deleted from the tree, it is skipped.
        std::fs::write(dir.join("tracked.rs"), b"\0").unwrap();
        assert_eq!(nul_bytes(&dir, &walked(&dir)).len(), 1);
        std::fs::remove_file(dir.join("tracked.rs")).unwrap();
        assert!(nul_bytes(&dir, &walked(&dir)).is_empty());
    }
}
