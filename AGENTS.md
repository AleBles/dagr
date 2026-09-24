# Working on Dagr

A GTK4/libadwaita to-do list in Rust: one window, a background service that
hosts an MCP endpoint and a Unix socket, and one SQLite file shared by both.

This file is for any coding agent. Claude Code, Codex and the rest all read
`AGENTS.md`; there is deliberately no tool-specific copy.

## Getting around

| Path | What lives there |
|---|---|
| `src/db.rs` | SQLite: schema, every query. No rules about *what* is allowed. |
| `src/api.rs` | Every task rule. `mcp.rs` and `serve.rs` are thin wrappers over it. |
| `src/mcp.rs`, `src/mcp_http.rs` | The MCP tools and their HTTP transport. |
| `src/proto.rs`, `src/serve.rs` | The socket protocol and the background service. |
| `src/settings.rs` | Typed settings, stored as key/value rows in SQLite. |
| `src/task.rs` | Domain types and the `!`/`#tag`/`@list` parser. |
| `src/ui/` | The window, the task row, and `preferences/` (one module per page). |
| `src/i18n.rs`, `locales/app.yml` | Language selection and every user-facing string. |
| `install.sh` | The one-line installer the README and site point at. |
| `build-aux/homebrew/dagr.rb` | The Homebrew formula; the live copy is in `alebles/homebrew-tap`. |

## House rules

- **Every user action is also an MCP tool.** A guard test in `mcp.rs` asserts
  the exact tool count, so adding an action without a tool fails the build.
- **New features ship switched off**, behind a setting, with their own
  Preferences page carrying the switch. With the switch off the app must look
  and behave exactly as it did before.
- **Pre-release: no migrations.** Change the schema in place; starting over
  means deleting `~/.local/share/dagr/dagr.db`. The one exception is the
  `list_id` backfill in `Db::init`, which exists so nobody had to.
- **The window does not go through `api.rs`.** It writes to `Db` directly, so a
  rule both front doors must share belongs in `db.rs` — that is why typed tags
  resolve there.
- **User-facing text goes through `tr!` and `locales/app.yml`**, English and
  Dutch. Text an assistant reads — MCP tool descriptions, API errors, CLI
  output — stays English on purpose: it is an interface contract. A test fails
  on any string missing a translation.
- **The UI rebuilds, it does not sync.** Every change writes to SQLite, reloads
  and rebuilds the list. Row callbacks defer through `Ctx::later` so a widget is
  never destroyed inside its own signal handler.
- **Linux and macOS both build.** The differences sit behind
  `cfg(target_os = "macos")` in three places: `paths.rs` (socket under
  `$TMPDIR`), `serve.rs` (launchd instead of systemd) and `i18n.rs`
  (`AppleLanguages`). CI runs clippy and the tests on both, so a platform-only
  item that goes unused on the other fails the build.
- Exhaustive struct literals in tests (`Settings`, the params `patch()` helper)
  break when a field is added. That is the point; update them.

## Before calling anything done

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings   # CI runs with -D warnings
cargo test --locked
```

## Running it without disturbing real data

Point the app at a scratch directory; it then has its own database, its own
background service and its own socket:

```bash
XDG_DATA_HOME=/tmp/scratch/data DAGR_SOCKET=/tmp/scratch/dagr.sock cargo run
```

On macOS `$XDG_DATA_HOME` works the same way; the socket defaults to
`$TMPDIR/dagr/` rather than a runtime directory.

Drive that instance over the socket with newline-delimited JSON (`add_task`,
`update_settings`, `list_tasks`, …) — the quickest way to set up state without
clicking. The window notices outside writes within a second.

Opening Preferences without a keyboard, for screenshots:

```bash
gdbus call --session --dest nu.bles.dagr --object-path /nu/bles/dagr/window/1 \
  --method org.gtk.Actions.Activate preferences "[]" "{}"
```

Anything needing a real click or keystroke — popovers, drag-and-drop, the
delete confirmation — cannot be automated here; say plainly that it was left
for a human rather than implying it was tested.

## Flatpak

To try `install.sh` without touching the real installation, give it a scratch
home and a scratch Flatpak user directory. It then downloads and installs the
real latest release into those; the GNOME runtime, if it is installed
system-wide, is shared rather than downloaded again:

```bash
env HOME=/tmp/scratch/home FLATPAK_USER_DIR=/tmp/scratch/flatpak bash install.sh
```

`./build-aux/install-flatpak.sh` builds and installs it. After changing
dependencies, regenerate the vendored sources or the sandboxed offline build
will fail:

```bash
python3 build-aux/generate-cargo-sources.py
```

## Releasing

Bump `version` in `Cargo.toml`, then push a matching tag:

```bash
git tag v0.3.0 && git push origin v0.3.0
```

`.github/workflows/release.yml` refuses a tag that disagrees with `Cargo.toml`,
builds the Flatpak bundle, and creates the GitHub release with it attached. It
then renders `build-aux/homebrew/dagr.rb` for the tag, attaches that too, and
pushes it to the tap — which needs the `HOMEBREW_TAP_TOKEN` secret.

## Style

Comments explain *why*, not *what*, and this is the author's first Rust
project: prefer the plain, readable version over the clever one, and keep new
code looking like the code around it.
