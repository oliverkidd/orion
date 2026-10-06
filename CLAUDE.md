# Working on orion

## Versions and releases

Every push to `main` is a release, and `.github/workflows/release.yml` picks its number from what changed since the last tag:

- **Minor** (x.Y.0): any path in `crates/orion-daemon/daemon-inputs.txt` changed. Upgrading across one restarts the daemon and every session in it.
- **Patch** (x.y.Z): anything else. The upgrade reopens orion and the running daemon keeps its sessions.
- **Major** (X.0.0): never automatic. When a change earns one (it breaks how people use orion, or drops something they relied on), raise `[workspace.package] version` in the root `Cargo.toml` to the next major in that same change, and say why in the commit message.

The same list is what `crates/orion-daemon/build.rs` hashes into the daemon's build stamp. When you add code the daemon is built from somewhere new, add its path there. A path missing from the list is daemon code that an upgrade would silently skip.
