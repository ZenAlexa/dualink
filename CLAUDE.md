# Repository Guidelines

- Repo: https://github.com/ZenAlexa/dualink
- Fork of `feschber/lan-mouse` — software KVM for keyboard/mouse sharing across machines

## Architecture

Core: daemon captures input on one machine, sends via DTLS/UDP, emulates on the other.
Platforms: macOS (CGEventTap/CGEventPost), Linux (libei/wlroots), Windows (SendInput).

### Crate Layout

- `src/` — main daemon binary (`service.rs` event loop, `config.rs`, `emulation.rs`)
- `input-capture/` — platform input capture backends
- `input-emulation/` — platform input emulation backends (CGEventPost, VirtualHID)
- `input-event/` — cross-platform event types + scancode definitions
- `lan-mouse-proto/` — network protocol (UDP + DTLS)
- `lan-mouse-ipc/` — IPC between daemon and frontends (Unix socket: `dualink.sock`)
- `src/clipboard/` + `src/clipboard_sync.rs` — clipboard sync over TCP (port 4243)

## Reference Repos

- `_reference/` — local clones of competitor KVM projects (gitignored)
- `./scripts/ref-sync.sh` — clone or update all reference repos
- Repos: deskflow, input-leap, barrier, lan-mouse (upstream)

## Build & Dev Commands

```sh
cargo build --no-default-features              # standard macOS build
cargo build --no-default-features --features macos_vhid  # with Karabiner VirtualHID
cargo test                                     # run all tests
cargo clippy --no-default-features -- -D warnings  # lint
cargo fmt --check                              # format check
```

## Key Decisions

- Config path: `~/.config/dualink/config.toml`
- Cert: `dualink.pem`
- GTK removed from default features (CLI-first on macOS)
- Key remapping: applied in `InputEmulation::consume()` before dispatch
- VirtualHID: behind `macos_vhid` feature flag, uses `karabiner-driverkit` crate
- Edge cooldown: 200ms in `input-capture/src/macos.rs`
- Env var: `DUALINK_LOG_LEVEL`

## Coding Style

- Rust 2021 edition. Follow `rustfmt` defaults.
- Error handling: `anyhow` for application code, `thiserror` for library crates.
- Async: `tokio` runtime. Use `select!` for concurrent operations.
- Platform-specific code behind `cfg` attributes, not runtime checks.
- Comments for non-obvious logic only. ALL English in code files.
