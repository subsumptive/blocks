# Releasing Subsumptive Blocks

## Release Checklist

1. Update `workspace.package.version` in `Cargo.toml`.
2. Run local checks:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   cargo build --release
   ```
3. Commit and tag the release:
   ```bash
   git tag -a v0.3.0 -m "Release v0.3.0"
   git push origin v0.3.0
   ```
4. `.github/workflows/release.yml` validates the tag, builds all 5 target binaries, generates `SHA256SUMS.txt`, and publishes the GitHub Release.
5. Sync `scripts/install.sh` and `scripts/install.ps1` to `subsumptive-website/public/` and deploy `subsumptive.net`.
6. Verify `https://subsumptive.net/install.sh` and `https://subsumptive.net/install.ps1` return HTTP 200 with script contents.
