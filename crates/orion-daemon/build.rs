//! Bakes ORION_DAEMON_FINGERPRINT into the build: a hash of the sources the
//! DAEMON is built from (`daemon-inputs.txt`), so two builds whose daemon code
//! is the same carry the same stamp whatever else changed between them. A
//! running daemon records it (`lifecycle::write_buildstamp`), and an upgrade
//! to a binary with the same stamp keeps that daemon and its sessions.
//!
//! The stamp must not move when only the version does — the release workflow
//! stamps its version into Cargo.toml and Cargo.lock before building — so
//! those two lines are left out of the hash.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../..");
    let list = manifest.join("daemon-inputs.txt");
    println!("cargo:rerun-if-changed={}", list.display());

    let mut files = Vec::new();
    for line in fs::read_to_string(&list)
        .expect("read daemon-inputs.txt")
        .lines()
    {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = root.join(line);
        assert!(
            path.exists(),
            "daemon-inputs.txt names {line}, which does not exist"
        );
        // A directory is watched recursively by cargo.
        println!("cargo:rerun-if-changed={}", path.display());
        collect(&root, Path::new(line), &mut files);
    }
    files.sort();
    files.dedup();

    let mut hash = Fnv::new();
    for rel in &files {
        let bytes = fs::read(root.join(rel)).expect("read daemon input");
        let name = rel.to_string_lossy().replace('\\', "/");
        hash.write(name.as_bytes());
        hash.write(&[0]);
        let bytes = match name.as_str() {
            "Cargo.toml" | "Cargo.lock" => without_workspace_version(&bytes),
            _ => bytes,
        };
        hash.write(&(bytes.len() as u64).to_le_bytes());
        hash.write(&bytes);
    }
    println!("cargo:rustc-env=ORION_DAEMON_FINGERPRINT={:016x}", hash.0);
}

/// Every file under `rel` (relative to `root`), skipping dot-entries and
/// build output.
fn collect(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
    let path = root.join(rel);
    if path.is_file() {
        out.push(rel.to_path_buf());
        return;
    }
    let Ok(entries) = fs::read_dir(&path) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        collect(root, &rel.join(&*name), out);
    }
}

/// The root Cargo.toml's `[workspace.package]` version, and each workspace
/// crate's version in Cargo.lock, dropped: the only lines a release stamp
/// rewrites. Everything else — dependencies, profiles — still counts.
fn without_workspace_version(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut after_workspace_crate = false;
    for line in text.lines() {
        let version = line.starts_with("version = ");
        // Cargo.toml: the one line that starts with `version = ` is the
        // workspace's (every dependency spells it inside braces). Cargo.lock:
        // the line after a workspace crate's `name = "orion…"`.
        let skip = version && (after_workspace_crate || !text.contains("[[package]]"));
        after_workspace_crate = line == "name = \"orion\"" || line.starts_with("name = \"orion-");
        if !skip {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.into_bytes()
}

/// FNV-1a, 64-bit: stable across toolchains, unlike std's `DefaultHasher`,
/// so a local build and a release build of the same daemon code agree.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
    }
}
