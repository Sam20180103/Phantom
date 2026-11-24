# Contributing to Phantom

Open an issue in the Phantom repository with a reproducible example for a
bug or a short proposal for a larger change. Small, focused fixes and
documentation updates can come directly as pull requests. Use
[SECURITY.md](SECURITY.md) for vulnerability reports.

For local setup, see [README.md](README.md). Read [TERAX.md](TERAX.md) for
the architecture and [INTEGRATION-v0.9.0.md](INTEGRATION-v0.9.0.md) for the
current integration status.

Run the checks relevant to your change from the repository root.

```sh
pnpm lint
pnpm check-types
pnpm test
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Describe how you verified the change in your pull request. Include platform
and runtime details when the change affects terminal, desktop, or browser
behavior. Changes to vendored code should preserve the relevant licenses
and update the upstream revision in the corresponding provenance document.

For changes specific to Terax, see the
[upstream contribution guide](https://github.com/crynta/terax-ai/blob/v0.9.0/CONTRIBUTING.md).
