# Phantom

Phantom is an open-source, terminal-first development workspace based on
[Terax v0.9.0](https://github.com/crynta/terax-ai/tree/v0.9.0). It adds a
headless Rust gateway so you can use the workspace from a browser and
coordinate coding agents across terminal sessions.

## What you can do

- Use split terminals, a code editor, a file explorer, Git tools, and web preview in one workspace.
- Run the workspace on a remote host with `phantom-server` and connect through a browser.
- Reconnect to persistent terminal sessions with rmux.
- Coordinate CLI agents in a shared Room, with messages and session status.
- Connect the AI panel to a supported provider with your own credentials or use a local model.

## Build from source

Install [Rust](https://rustup.rs/), Node.js 22 or newer, and the pnpm version
specified in [package.json](package.json). Desktop builds also require the
[Tauri platform prerequisites](https://tauri.app/start/prerequisites/).

```sh
pnpm install --frozen-lockfile
pnpm run build
cargo build --locked -p phantom-server --release
```

For browser access, follow the [server setup guide](crates/phantom-server/README.md)
to configure the listener, authentication, and TLS. Run the server from the
repository root so it can serve the built UI from `dist/`.

For desktop development, run `pnpm tauri dev`. To build a desktop bundle,
run `pnpm tauri build`.

For architecture and current integration status, see [TERAX.md](TERAX.md)
and [INTEGRATION-v0.9.0.md](INTEGRATION-v0.9.0.md). See
[CONTRIBUTING.md](CONTRIBUTING.md) for contribution and verification steps.

## Upstream and acknowledgments

Phantom is an independently maintained fork of [Terax](https://github.com/crynta/terax-ai).
Terax provides the foundation for the terminal, editor, file explorer, Git
interface, and AI workspace. Phantom extends that foundation with
`phantom-core`, the browser and WebSocket gateway, persistent remote PTYs,
and Room-based agent collaboration.

Thank you to [crynta](https://github.com/crynta) and the Terax contributors
for building Terax and sharing it as open source. For official Terax
releases and documentation, see the [upstream repository](https://github.com/crynta/terax-ai)
and [Terax documentation](https://terax.app/docs).

## License

Phantom is distributed under the [Apache License 2.0](LICENSE), except
where files or components identify their own licenses. See [NOTICE](NOTICE)
for upstream attribution and the licenses of bundled terminal components.
