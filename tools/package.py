#!/usr/bin/env python3
"""Package greenhouse as a `.hnmod`, the source-only archive Horizon's launcher installs.

The archive holds `horizon-native.json` and `mixins/<id>/` (only `.rs` and `.toml`), with fixed
dates and modes, so the same sources always produce the same bytes and the same SHA-256. The
launcher reads it without running anything and compiles it on the pinned Horizon revision.

Written to run on Python 3.9 (no `tomllib`), because that is what the development machine had;
it parses only the two fields it needs out of `horizon-mixin.toml`.
"""
import hashlib
import json
import re
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GAME = "26.3"


def section_values(path, section):
    """The `key = "value"` pairs of one TOML table, without a TOML parser."""
    values, current = {}, None
    for line in path.read_text().splitlines():
        header = re.match(r"^\s*\[([^\[\]]+)\]\s*$", line)
        if header:
            current = header.group(1).strip()
            continue
        pair = re.match(r'^\s*([A-Za-z0-9_-]+)\s*=\s*"([^"]*)"\s*$', line)
        if pair and current == section:
            values[pair.group(1)] = pair.group(2)
    return values


def main():
    compat = json.loads((ROOT / "compatibility.json").read_text())
    manifests = sorted((ROOT / "mixins").glob("*/horizon-mixin.toml"))
    if len(manifests) != 1:
        sys.exit("a .hnmod holds one bundle: keep exactly one mixins/<id>/horizon-mixin.toml")
    manifest_path = manifests[0]
    bundle = manifest_path.parent
    mod = section_values(manifest_path, "mod")
    if "id" not in mod or "version" not in mod:
        sys.exit("horizon-mixin.toml has no [mod] id or version")
    if bundle.name != mod["id"]:
        sys.exit(f"rename mixins/{bundle.name}/ to mixins/{mod['id']}/ (the registry finds it by id)")
    if not re.fullmatch(r"[a-z0-9_]+", mod["id"]):
        sys.exit("the registry takes ids of lowercase letters, digits and _ only")

    pinned = section_values(manifest_path, "compatibility").get("horizon_revision")
    if pinned != compat["horizon_revision"]:
        sys.exit("horizon-mixin.toml and compatibility.json pin different Horizon revisions")

    manifest = {
        "schema": 1,
        "id": mod["id"],
        "version": mod["version"],
        "game": GAME,
        "horizon": compat["horizon_revision"],
        "mixins": compat["horizon_mixins_revision"],
    }
    files = {"horizon-native.json": (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode()}
    for path in sorted(bundle.iterdir()):
        if path.is_file() and path.suffix in (".rs", ".toml"):
            files[path.relative_to(ROOT).as_posix()] = path.read_bytes()

    out = ROOT / "target" / f"{mod['id']}-{mod['version']}.hnmod"
    out.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, content in sorted(files.items()):
            info = zipfile.ZipInfo(name, (2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, content)

    print(json.dumps({
        "file": str(out),
        "size": out.stat().st_size,
        "sha256": hashlib.sha256(out.read_bytes()).hexdigest(),
        "contents": sorted(files),
    }, indent=2))


if __name__ == "__main__":
    main()
