# Contributing

Bug reports, documentation fixes, and focused pull requests are welcome. For a larger change, open an issue first so we can agree on the scope.

Read the [development guide](docs/local-development.md) before building locally. The workspace requires Rust 1.89 or newer.

Before opening a pull request, run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast --locked
cargo run -p jevalan -- lab check
cargo run -p jevalan -- lab offline
```

Keep changes focused, add or update tests for behavior changes, and update the documentation when commands or configuration change. Never include credentials, `.env` files, local configuration, or `.local/` artifacts in a pull request.
