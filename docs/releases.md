# Release policy

The package version in `src/crates/jevalan/Cargo.toml` is the source of truth. A Git tag uses the same
version with a `v` prefix: `0.1.0-alpha.1` is released as `v0.1.0-alpha.1`.

## Alpha

- Alpha releases are previews. Commands, configuration, and protocol details may change between
  alpha versions; users should keep their data and configuration under version control.
- Increment the alpha number for each published preview (`0.1.0-alpha.1`, `0.1.0-alpha.2`, …).
- An alpha tag creates a GitHub prerelease. It is not published to crates.io.
- The release workflow runs the workspace tests, builds the CLI, and attaches archives for Windows
  x86_64, Linux x86_64, macOS x86_64, and macOS arm64. Each archive includes the executable,
  example configuration, runtime flows, capabilities, signals, lab fixtures, README, and licenses.
- Extract an archive and run jevalan from its top-level directory so the bundled relative paths
  resolve.
- Create a release only from a commit whose changes and tests have been reviewed. Never reuse or
  move a published tag; fix a release by publishing the next alpha number.

## Stable releases

Use SemVer after the public commands, configuration, and protocols are stable. `1.0.0` marks that
commitment. After `1.0.0`, patch releases contain compatible fixes, minor releases add compatible
features, and breaking changes require a major version. Until then, alpha versions make no
compatibility promise.

## Publish

1. Update `src/crates/jevalan/Cargo.toml` to the release version and update the README status.
2. Run `cargo test --workspace --locked` and review the changes.
3. Commit the release changes, then create and push the matching tag:

   ```powershell
   git tag v0.1.0-alpha.1
   git push origin v0.1.0-alpha.1
   ```

4. Wait for the **Release** workflow. It publishes only after tests and all four native builds pass.
   If a job fails, fix it and publish a new versioned tag; do not move the existing tag.

The archives are unsigned and macOS builds are not notarized. Windows or macOS may show an
operating-system warning on first launch.
