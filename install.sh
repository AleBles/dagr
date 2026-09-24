#!/usr/bin/env bash
# Installs Dagr for the current user: the Flatpak bundle from the latest GitHub
# release on Linux, the Homebrew formula on macOS. No sudo. Run it again to
# update.
#
#   curl -fsSL https://raw.githubusercontent.com/AleBles/dagr/main/install.sh | bash
#
# Everything below is a function and `main` runs on the very last line, so a
# download cut off halfway can never run half a script.

set -euo pipefail

APP_ID="nu.bles.dagr"
BUNDLE_URL="https://github.com/AleBles/dagr/releases/latest/download/dagr.flatpak"
FLATHUB="https://dl.flathub.org/repo/flathub.flatpakrepo"
TAP_FORMULA="alebles/tap/dagr"

if [ -t 1 ]; then
    BOLD=$'\033[1m' DIM=$'\033[2m' RESET=$'\033[0m'
else
    BOLD="" DIM="" RESET=""
fi

step() { printf '%s==>%s %s\n' "$BOLD" "$RESET" "$*"; }
note() { printf '    %s%s%s\n' "$DIM" "$*" "$RESET"; }
fail() {
    printf 'dagr install: %s\n' "$*" >&2
    exit 1
}
need() { command -v "$1" >/dev/null 2>&1; }

# --- Linux: the Flatpak bundle ---------------------------------------------

install_linux() {
    need flatpak || fail "Flatpak is needed first, from your package manager
    (sudo dnf install flatpak, sudo apt install flatpak, sudo pacman -S flatpak),
    then run this again."
    need curl || fail "curl is needed to download the release."

    step "Adding Flathub, for the GNOME runtime the bundle runs on"
    note "Dagr itself does not come from Flathub."
    flatpak remote-add --user --if-not-exists flathub "$FLATHUB"

    local tmp
    tmp=$(mktemp -d)
    # shellcheck disable=SC2064 # expand now: tmp is local and gone by exit
    trap "rm -rf '$tmp'" EXIT

    step "Downloading the latest release"
    curl -fL --progress-bar "$BUNDLE_URL" -o "$tmp/dagr.flatpak"

    step "Installing"
    # -y matters twice over: it answers the runtime prompt, and under
    # `curl | bash` the script itself is on stdin, so a prompt would eat it.
    flatpak install --user -y --reinstall "$tmp/dagr.flatpak"

    install_shim
}

# A Flatpak puts no command on PATH, and every `dagr ...` in the docs assumes
# one. Ours is a two-line wrapper; anything else already there is left alone.
install_shim() {
    local bin="$HOME/.local/bin" shim
    shim="$bin/dagr"
    local wrapper before_rename current=""
    wrapper=$(printf '#!/bin/sh\nexec flatpak run %s "$@"\n' "$APP_ID")
    # The same wrapper for the app id Dagr had until 0.2.0, which is what
    # anyone who followed the older README still has.
    before_rename=$(printf '#!/bin/sh\nexec flatpak run %s "$@"\n' "dev.ables.Dagr")
    [ -e "$shim" ] && current=$(cat "$shim")

    if [ -e "$shim" ] && [ "$current" != "$wrapper" ] && [ "$current" != "$before_rename" ]; then
        step "Leaving $shim alone"
        note "It is not the wrapper this script writes; use 'flatpak run $APP_ID'"
        note "or remove it and run this again."
        return
    fi

    if [ "$current" = "$wrapper" ]; then
        step "The dagr command is already in $bin"
        return
    fi

    if [ "$current" = "$before_rename" ]; then
        step "Pointing the dagr command at $APP_ID"
        note "It still ran dev.ables.Dagr, the app id from before 0.2.0."
        # Not removed for you: its tasks are in its own data directory.
        if flatpak info --user dev.ables.Dagr >/dev/null 2>&1; then
            note "That old app is still installed, with its own tasks in"
            note "$HOME/.var/app/dev.ables.Dagr; remove it when you no longer need them:"
            note "flatpak uninstall --user dev.ables.Dagr"
        fi
    else
        step "Adding the dagr command to $bin"
    fi
    mkdir -p "$bin"
    printf '%s\n' "$wrapper" >"$shim"
    chmod +x "$shim"
    case ":$PATH:" in
    *":$bin:"*) ;;
    *) note "$bin is not on your PATH yet; add it to use 'dagr' from a terminal." ;;
    esac
}

# --- macOS: the Homebrew tap -----------------------------------------------

install_macos() {
    need brew || fail "Homebrew is needed first: https://brew.sh - then run this again."

    step "Installing $TAP_FORMULA"
    note "Homebrew builds it from source, so the first install takes a few minutes."
    # Installs, or upgrades an installed copy that is out of date.
    brew install "$TAP_FORMULA"

    # Spotlight and Launchpad only find app bundles. The copy is a launcher
    # for the Homebrew binary, so later upgrades need no new one.
    local apps="$HOME/Applications"
    step "Copying Dagr.app to $apps"
    mkdir -p "$apps"
    rm -rf "$apps/Dagr.app"
    cp -R "$(brew --prefix)/opt/dagr/Dagr.app" "$apps/"
}

# ---------------------------------------------------------------------------

next_steps() {
    local service="dagr setup     # prints how to start it with your session"
    if [ "$1" = Darwin ]; then
        service="brew services start dagr"
    fi
    printf '\n%sDagr is installed.%s Open it from your launcher, or run:\n\n' "$BOLD" "$RESET"
    printf '    dagr\n\n'
    printf 'To keep the background service (and the MCP endpoint) running at login:\n\n'
    printf '    %s\n\n' "$service"
}

main() {
    if [ "$(id -u)" -eq 0 ]; then
        fail "run this as yourself, not as root: it installs for your user only."
    fi
    local os
    os=$(uname -s)
    case "$os" in
    Linux) install_linux ;;
    Darwin) install_macos ;;
    *) fail "Dagr runs on Linux and macOS; this is $os." ;;
    esac
    next_steps "$os"
}

main "$@"
