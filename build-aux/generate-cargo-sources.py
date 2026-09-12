#!/usr/bin/env python3
"""Turn Cargo.lock into flatpak-builder sources so the sandboxed build can run
`cargo --offline`.

A dependency-free stand-in for flatpak-builder-tools' flatpak-cargo-generator
that covers what this project needs: crates from crates.io. It fails loudly if
a git dependency ever shows up, at which point switch to the official tool.

Usage: build-aux/generate-cargo-sources.py [Cargo.lock] [-o build-aux/cargo-sources.json]
"""

import argparse
import json
import sys
import tomllib

CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
VENDOR_DIR = "cargo/vendor"

CARGO_CONFIG = """[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "cargo/vendor"
"""


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("lockfile", nargs="?", default="Cargo.lock")
    ap.add_argument("-o", "--output", default="build-aux/cargo-sources.json")
    args = ap.parse_args()

    with open(args.lockfile, "rb") as f:
        lock = tomllib.load(f)

    sources = []
    for pkg in lock["package"]:
        source = pkg.get("source")
        if source is None:
            continue  # the workspace crate itself
        if source != CRATES_IO:
            print(f"unsupported source for {pkg['name']}: {source}", file=sys.stderr)
            print("use flatpak-builder-tools/cargo/flatpak-cargo-generator.py instead", file=sys.stderr)
            return 1
        name, version, checksum = pkg["name"], pkg["version"], pkg["checksum"]
        dest = f"{VENDOR_DIR}/{name}-{version}"
        sources.append({
            "type": "archive",
            "archive-type": "tar-gzip",
            "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
            "sha256": checksum,
            "dest": dest,
        })
        sources.append({
            "type": "inline",
            "contents": json.dumps({"package": checksum, "files": {}}),
            "dest": dest,
            "dest-filename": ".cargo-checksum.json",
        })

    sources.append({
        "type": "inline",
        "contents": CARGO_CONFIG,
        "dest": "cargo",
        "dest-filename": "config.toml",
    })

    with open(args.output, "w") as f:
        json.dump(sources, f, indent=2)
        f.write("\n")
    print(f"wrote {args.output}: {len(sources)} entries for {(len(sources) - 1) // 2} crates")
    return 0


if __name__ == "__main__":
    sys.exit(main())
