#!/usr/bin/env python3
"""Check that every place carrying the release version agrees.

The version lives in Cargo.toml, the applet's metadata.json, and the release
PKGBUILD. paru decides whether `kde-ai-usage-monitor` has an update by reading
the PKGBUILD's pkgver, and the release workflow tags `v<version>` from
Cargo.toml, so a bump that misses one of them either never reaches users or
points the PKGBUILD at a tag that does not exist.

    python3 scripts/check-version.py            # files agree with each other
    python3 scripts/check-version.py v0.2.0     # ...and with this tag

Prints the version on success, so the release workflow can read it.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def cargo_version() -> str | None:
    text = (ROOT / "Cargo.toml").read_text()
    package = re.search(r"^\[package\](.*?)(?=^\[|\Z)", text, flags=re.M | re.S)
    match = package and re.search(r'^version\s*=\s*"([^"]+)"', package.group(1), flags=re.M)
    return match.group(1) if match else None


def applet_version() -> str | None:
    metadata = json.loads((ROOT / "plasmoid" / "package" / "metadata.json").read_text())
    return metadata.get("KPlugin", {}).get("Version")


def pkgbuild_version() -> str | None:
    text = (ROOT / "packaging" / "arch" / "kde-ai-usage-monitor" / "PKGBUILD").read_text()
    match = re.search(r"^pkgver=(\S+)$", text, flags=re.M)
    return match.group(1) if match else None


def main() -> int:
    versions = {
        "Cargo.toml": cargo_version(),
        "plasmoid/package/metadata.json": applet_version(),
        "packaging/arch/kde-ai-usage-monitor/PKGBUILD": pkgbuild_version(),
    }
    if len(sys.argv) > 1:
        versions[f"tag {sys.argv[1]}"] = sys.argv[1].removeprefix("v")

    if len(set(versions.values())) != 1 or None in versions.values():
        print("version mismatch:", file=sys.stderr)
        for source, version in versions.items():
            print(f"  {source}: {version}", file=sys.stderr)
        return 1

    print(versions["Cargo.toml"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
