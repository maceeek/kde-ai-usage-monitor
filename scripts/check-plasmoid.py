#!/usr/bin/env python3
"""Static checks for the Plasma applet.

Plasma's QML modules are not installable on a CI runner, so `qmllint` cannot
resolve a single `org.kde.*` import here and has nothing useful to say. These
checks need no Qt at all and catch the mistakes that actually bite in this
package: a config page bound to a setting that does not exist, a config
category pointing at a missing file, or an unbalanced brace.

Run it by hand with `python3 scripts/check-plasmoid.py`.
"""

from __future__ import annotations

import json
import re
import sys
import xml.etree.ElementTree as ElementTree
from pathlib import Path

PACKAGE = Path(__file__).resolve().parent.parent / "plasmoid" / "package"
CONTENTS = PACKAGE / "contents"

REQUIRED_FILES = [
    PACKAGE / "metadata.json",
    CONTENTS / "ui" / "main.qml",
    CONTENTS / "config" / "config.qml",
    CONTENTS / "config" / "main.xml",
]

failures: list[str] = []


def fail(message: str) -> None:
    failures.append(message)


def strip_literals(source: str) -> str:
    """Remove comments and string literals so their brackets do not count."""
    source = re.sub(r"//[^\n]*", "", source)
    source = re.sub(r"/\*.*?\*/", "", source, flags=re.S)
    source = re.sub(r'"(?:[^"\\\n]|\\.)*"', '""', source)
    source = re.sub(r"'(?:[^'\\\n]|\\.)*'", "''", source)
    return source


def check_required_files() -> None:
    for path in REQUIRED_FILES:
        if not path.is_file():
            fail(f"missing required file: {path.relative_to(PACKAGE.parent)}")


def check_metadata() -> None:
    path = PACKAGE / "metadata.json"
    if not path.is_file():
        return
    try:
        metadata = json.loads(path.read_text())
    except json.JSONDecodeError as error:
        fail(f"metadata.json is not valid JSON: {error}")
        return

    if metadata.get("KPackageStructure") != "Plasma/Applet":
        fail('metadata.json: KPackageStructure must be "Plasma/Applet"')

    plugin = metadata.get("KPlugin", {})
    for key in ("Id", "Name", "Version", "Icon", "License"):
        if not plugin.get(key):
            fail(f"metadata.json: KPlugin.{key} is missing or empty")


def check_brackets() -> None:
    for path in sorted(CONTENTS.rglob("*.qml")) + sorted(CONTENTS.rglob("*.js")):
        source = strip_literals(path.read_text())
        for opener, closer in (("{", "}"), ("(", ")"), ("[", "]")):
            if source.count(opener) != source.count(closer):
                fail(
                    f"{path.relative_to(PACKAGE.parent)}: unbalanced '{opener}{closer}' "
                    f"({source.count(opener)} vs {source.count(closer)})"
                )


def config_entry_names() -> set[str]:
    path = CONTENTS / "config" / "main.xml"
    if not path.is_file():
        return set()
    try:
        root = ElementTree.parse(path).getroot()
    except ElementTree.ParseError as error:
        fail(f"config/main.xml is not valid XML: {error}")
        return set()
    # The KCfg schema puts everything in a namespace, so match on the tag's
    # local name rather than spelling the namespace out.
    return {
        entry.attrib["name"]
        for entry in root.iter()
        if entry.tag.rsplit("}", 1)[-1] == "entry" and "name" in entry.attrib
    }


def check_config_bindings(entries: set[str]) -> None:
    """Every `cfg_<key>` a config page declares must exist in main.xml.

    Plasma binds these by name and silently ignores the ones it cannot match,
    so a typo here means a setting that appears to save and never does.
    """
    used: set[str] = set()
    for path in sorted((CONTENTS / "ui").glob("Config*.qml")):
        for key in re.findall(r"\bcfg_([A-Za-z_][A-Za-z0-9_]*)\b", path.read_text()):
            used.add(key)
            if key not in entries:
                fail(
                    f"{path.relative_to(PACKAGE.parent)}: cfg_{key} has no matching "
                    "<entry> in config/main.xml"
                )

    for unused in sorted(entries - used):
        print(f"note: config entry '{unused}' is not bound by any config page")


def check_config_sources() -> None:
    path = CONTENTS / "config" / "config.qml"
    if not path.is_file():
        return
    for source in re.findall(r'source:\s*"([^"]+)"', path.read_text()):
        if not (CONTENTS / "ui" / source).is_file():
            fail(f"config/config.qml: category source '{source}' does not exist in ui/")


def check_local_components() -> None:
    """QML types named after a sibling file must have that file present."""
    available = {path.stem for path in (CONTENTS / "ui").glob("*.qml")}
    for path in sorted((CONTENTS / "ui").glob("*.qml")):
        source = strip_literals(path.read_text())
        # A local component is used as `Name {` with nothing qualifying it.
        for name in re.findall(r"(?<![.\w])([A-Z][A-Za-z0-9]*)\s*\{", source):
            if name in available or name == path.stem:
                continue
            # Anything else is expected to come from an import; only flag names
            # that look like this package's own files.
            if name.startswith("Config") or name.endswith("Representation"):
                fail(f"{path.relative_to(PACKAGE.parent)}: uses {name}, but ui/{name}.qml is missing")


def main() -> int:
    if not PACKAGE.is_dir():
        print(f"error: {PACKAGE} does not exist", file=sys.stderr)
        return 1

    check_required_files()
    check_metadata()
    check_brackets()
    check_config_bindings(config_entry_names())
    check_config_sources()
    check_local_components()

    if failures:
        print(f"\n{len(failures)} problem(s) found:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    checked = len(list(CONTENTS.rglob("*.qml"))) + len(list(CONTENTS.rglob("*.js")))
    print(f"applet package OK ({checked} QML/JS files checked)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
