<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="site/logo-dark.svg">
    <img src="site/logo.svg" alt="dagr" width="300">
  </picture>
</p>

<p align="center">
  <a href="https://github.com/alebles/dagr/actions/workflows/ci.yml"><img src="https://github.com/alebles/dagr/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/built%20with-Rust-dea584.svg" alt="Rust"></a>
  <a href="https://dagr.bles.nu"><img src="https://img.shields.io/badge/site-dagr.bles.nu-d97706.svg" alt="Site"></a>
</p>

A tiny, keyboard-first task list for Linux. One window, one priority-ordered list, and everything a single click away - plus a built-in MCP server, so an AI assistant can do anything you can.

> _"Skinfaxi they name the steed that draws the shining day over mankind — brightest of horses he seems to men, and ever his mane is aflame with light."_
> — Vafþrúðnismál, of the horse that bears Dagr across the sky

<p align="center">
  <img src="site/screenshot.png" alt="Dagr window" width="640">
</p>

## Features

- **One click for everything** - complete, rename, re-prioritize, or delete without a menu or a dialog in sight.
- **Priority-ordered** - user-defined levels with names and colors; the order you set is the order the list sorts. Reorder by dragging or with the arrows.
- **Type to add** - the entry is focused on launch; a leading `!`, `!!`, `!!!` bumps the new task's priority.
- **Undo, not confirm** - deletes and "clear completed" drop a toast with Undo instead of asking first.
- **Adjustable** - turn priorities off entirely, choose the sort order and the default priority, all in Preferences.
- **AI-native** - the same binary is an MCP server over stdio or local HTTP, sharing one database with the open window.
- **Native GNOME look** - GTK 4 + libadwaita, following the system light/dark style and accent color. Runs on any Linux desktop (developed on Hyprland), no GNOME shell required.
- **Local and private** - a single SQLite file in your data directory. No account, no cloud, no telemetry.

## Install

Build from source with a Rust toolchain and the GTK 4 / libadwaita development files.

**Fedora**

```bash
sudo dnf install rust cargo gtk4-devel libadwaita-devel
cargo run --release
```

**Debian / Ubuntu**

```bash
sudo apt install cargo libgtk-4-dev libadwaita-1-dev
cargo run --release
```

**Arch**

```bash
sudo pacman -S rust gtk4 libadwaita
cargo run --release
```

Needs GTK ≥ 4.18 and libadwaita ≥ 1.7 (the versions the bindings are gated to; the code itself only uses APIs from GTK 4.12 / libadwaita 1.5). Tasks live in `~/.local/share/dagr/dagr.db`.

To install a launcher entry:

```bash
cargo install --path .
install -Dm644 data/dev.ables.Dagr.desktop ~/.local/share/applications/
```

### Flatpak

```bash
flatpak install --user flathub org.gnome.Sdk//50 \
  org.freedesktop.Sdk.Extension.rust-stable//25.08 org.flatpak.Builder
flatpak run org.flatpak.Builder --user --install --force-clean \
  build-aux/build-dir build-aux/dev.ables.Dagr.json
flatpak run dev.ables.Dagr
```

Crates are vendored for the offline sandbox build in `build-aux/cargo-sources.json`; regenerate it after changing `Cargo.lock` with `build-aux/generate-cargo-sources.py`.

## Usage

The window opens with the entry focused. Type a task, press Enter, and keep going.

| Action | How |
|---|---|
| Add a task | Type and press Enter (`!`, `!!`, `!!!` set the priority) |
| Complete | Click the checkbox - it sinks to the bottom, struck through |
| Rename | Click the title, edit, Enter (Esc cancels) |
| Re-prioritize | Click the colored dot, pick a level |
| Reorder priorities | Drag the grip, or use the arrows, in Preferences |
| Delete | Click the trash icon - a toast offers Undo |
| Clear completed | Button in the header bar when anything is done |

**Shortcuts:** `Ctrl+N` focus the entry, `Ctrl+Shift+D` clear completed, `Ctrl+,` preferences, `Ctrl+Q` quit.

**Preferences** covers what to make adjustable: switch priorities on or off, choose the sort order (priority, oldest, newest, or alphabetical), set the default priority for new tasks, and manage the priority levels themselves.

## AI access (MCP)

`dagr --mcp` speaks the [Model Context Protocol](https://modelcontextprotocol.io) over stdio instead of opening a window, on the same database - so changes show up in an open window within a second.

Every action in the app is a tool: `list_tasks`, `add_task`, `update_task`, `delete_task`, `clear_completed`, `list_priorities`, `add_priority`, `update_priority`, `reorder_priorities`, `delete_priority`, `get_settings`, `update_settings`. Priorities can be given by name or id.

This repo ships a project-scoped `.mcp.json`, so opening Claude Code here just works. From anywhere:

```bash
cargo install --path .
claude mcp add --scope user dagr -- dagr --mcp
```

**HTTP mode** - Preferences → AI access starts a Streamable HTTP endpoint on `http://127.0.0.1:<port>/mcp` for as long as the window is open, bound to loopback with no authentication. Register it with:

```bash
claude mcp add --transport http dagr http://127.0.0.1:7331/mcp
```

## Development

```bash
cargo test                                # unit + integration tests
cargo clippy --all-targets -- -D warnings # what CI enforces
cargo fmt --check
```

Integration tests in `tests/` drive the real binary over MCP (stdio and HTTP) and need no display. CI runs fmt, clippy, and the tests on every push, and builds a Flatpak bundle.

## License

MIT - see [LICENSE](LICENSE).
