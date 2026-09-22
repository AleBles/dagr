# Changelog

Notable changes to Dagr, newest first. Features only — the reasoning behind
them lives in the commits.

Nothing has been released yet, so everything below is unreleased; the dated
headings are the points the work landed in the repository.

## 2026-09-22 — major UX improvements

### Added

- **Lists.** An optional sidebar of task lists with "All tasks" on top. Add,
  rename, reorder and remove them there; deleting a list moves its tasks rather
  than throwing them away, and asks first. Type `@list` in front of a new task
  to file it. Off by default.
- **Labels.** Optional colored tags, any number per task, shown as dots on the
  row with a picker behind them. Type `#tag` in front of a new task to attach
  one, creating it if it is new. Managed in Preferences. Off by default.
- **English and Dutch.** The app follows the system language, with an override
  in Preferences that applies on the next start.
- **Date before priority.** An optional second ordering: whole days move as a
  block, oldest first, and priority orders the tasks within a day.
- **Window size.** The size the window opens at is now a preference.
- **Keyboard shortcuts for the Preferences pages**, `Ctrl+1` to `Ctrl+5`.
- **A word when a typed marker is ignored.** Typing `!`, `#tag` or `@list`
  while that feature is switched off now says so in a toast, with a button
  straight to the switch.
- **Nine more ways in for assistants.** MCP grew from twelve tools to
  twenty-one, covering labels and lists, and the socket protocol gained the
  matching methods.

### Changed

- **Preferences is now a dialog with a sidebar**, one page per topic
  (Settings, Priorities, Labels, Lists, AI access) instead of one long page.
- **The app id is now `nu.bles.dagr`**, matching the domain the site lives on.
  A Flatpak installed under the old id has to be removed by hand.

## 2026-09-15 — the background service

- `dagr serve` runs without a window, hosting the MCP endpoint over HTTP and
  answering a Unix socket, so assistants and the desktop keep working after the
  window is closed.
- The stdio MCP transport was removed; there is one MCP server now.

## 2026-09-12 — first working app

- One window, one priority-ordered task list, everything a single click away.
- User-defined priorities with names and colors, reorderable, switchable off.
- Undo instead of confirmation for deletes and "clear completed".
- SQLite storage, Flatpak packaging, and an MCP server so an assistant can do
  everything the window can.
