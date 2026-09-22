#!/usr/bin/env bash
# Build the Flatpak from build-aux/nu.bles.dagr.json and install it into the
# user's Flatpak installation, so `flatpak run nu.bles.dagr` gets this build.
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec flatpak run org.flatpak.Builder \
    --user --install --force-clean \
    "$project_dir/_build" \
    "$project_dir/build-aux/nu.bles.dagr.json"
