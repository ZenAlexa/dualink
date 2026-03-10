# Dualink

Dualink is a silky-smooth software KVM for sharing mouse and keyboard across macOS and Windows machines.

Forked from [lan-mouse](https://github.com/feschber/lan-mouse), Dualink adds:
- **Karabiner-Elements compatibility** via VirtualHID input injection (replacing CGEventPost)
- **Modifier key remapping** for Win→Mac workflows (Ctrl→Cmd, Alt→Control, Win→Option)
- **Clipboard sync** with lazy pull over TCP
- **CLI-first** design (no GTK dependency)

## Features

- DTLS-encrypted UDP event pipeline for ultra-low latency
- Cross-platform: macOS (Apple Silicon + Intel) ↔ Windows
- Edge detection for seamless cursor transition between machines
- Configurable key remapping via `config.toml`

## Installation

### macOS (Apple Silicon / Intel)

```sh
# Prerequisites
# - Karabiner-Elements (for VirtualHID backend, optional)

# Build from source
cargo build --release --no-default-features

# Run daemon
./target/release/dualink daemon
```

### Windows

```sh
cargo build --release --no-default-features
dualink.exe daemon
```

## Configuration

Config file: `~/.config/dualink/config.toml` (macOS) or `%LOCALAPPDATA%\dualink\config.toml` (Windows)

```toml
# Listen port (default: 4242)
port = 4242

# Modifier key remapping (Win→Mac layout)
[key_remap]
KeyLeftCtrl = "KeyLeftMeta"      # Ctrl → Cmd
KeyRightCtrl = "KeyRightmeta"   # Right Ctrl → Right Cmd
KeyLeftAlt = "KeyLeftCtrl"       # Alt → Control
KeyRightalt = "KeyRightCtrl"    # Right Alt → Right Control
KeyLeftMeta = "KeyLeftAlt"       # Win → Option
KeyRightmeta = "KeyRightalt"    # Right Win → Right Option

# Release bind (return control to host)
release_bind = ["KeyLeftCtrl", "KeyLeftShift", "KeyLeftMeta", "KeyLeftAlt"]

# Authorized TLS certificate fingerprints
[authorized_fingerprints]
"aa:bb:cc:..." = "my-windows-pc"

# Remote machines
[[clients]]
position = "right"
hostname = "windows-pc"
activate_on_startup = true
ips = ["192.168.1.100"]
```

## CLI Usage

```sh
# Start daemon
dualink daemon

# CLI commands
dualink cli help
dualink cli list
dualink cli add --hostname windows-pc --position right
```

## Architecture

Dualink keeps lan-mouse's proven UDP+DTLS event pipeline intact. Three surgical additions:

1. **VirtualHID backend** (macOS): Replaces `CGEventPost` with Karabiner-DriverKit-VirtualHIDDevice for HID-level input injection compatible with Karabiner-Elements
2. **Key remapping**: Translates modifier keys in the emulation layer (Ctrl→Cmd, Alt→Control, Win→Option)
3. **Clipboard sync**: Lazy pull over TCP — copy notifications are instant, data transfers on paste

## Encryption

All network traffic is encrypted using DTLS via [WebRTC.rs](https://github.com/webrtc-rs/webrtc).

## License

GPL-3.0-or-later — same as the upstream project.

## Acknowledgements

Dualink is built on the excellent work of the [lan-mouse](https://github.com/feschber/lan-mouse) project by [@feschber](https://github.com/feschber). The core UDP+DTLS event pipeline, cross-platform input capture/emulation architecture, and edge-detection logic all originate from lan-mouse. Thank you for creating such a well-engineered and maintainable codebase.

Additional thanks to:
- [Karabiner-Elements](https://karabiner-elements.pqrs.org/) by [@tekezo](https://github.com/tekezo) — the VirtualHID DriverKit that makes Karabiner-compatible input injection possible
- [karabiner-driverkit](https://github.com/Psych3r/driverkit) by [@Psych3r](https://github.com/Psych3r) — Rust bindings for the Karabiner VirtualHID driver (from the [kanata](https://github.com/jtroo/kanata) ecosystem)
- [WebRTC.rs](https://github.com/webrtc-rs/webrtc) — DTLS encryption for the event pipeline
