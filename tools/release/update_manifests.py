#!/usr/bin/env python3
"""Render Homebrew/Scoop/Winget templates from a release manifest."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TEMPLATES = ROOT / "packaging" / "templates"
OUTPUT = ROOT / "packaging" / "out"


def _load_manifest(path: Path) -> dict:
    return json.loads(path.read_text())


def _find(artifacts: list[dict], name: str, platform: str, arch: str) -> dict:
    for item in artifacts:
        if (
            item["name"] == name
            and item["platform"] == platform
            and item["arch"] == arch
        ):
            return item
    raise SystemExit(f"artifact not found: {name} {platform} {arch}")


def _render(template_path: Path, out_path: Path, mapping: dict[str, str]) -> None:
    content = template_path.read_text()
    for key, value in mapping.items():
        content = content.replace(f"{{{{{key}}}}}", value)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(content)


def _render_homebrew(artifacts: list[dict], version: str) -> None:
    for name in ("molt", "molt-worker"):
        mac_arm = _find(artifacts, name, "macos", "arm64")
        mac_x86 = _find(artifacts, name, "macos", "x86_64")
        linux_arm = _find(artifacts, name, "linux", "aarch64")
        linux_x86 = _find(artifacts, name, "linux", "x86_64")
        mapping = {
            "VERSION": version,
            "MAC_ARM_URL": mac_arm["url"],
            "MAC_ARM_SHA256": mac_arm["sha256"],
            "MAC_X86_URL": mac_x86["url"],
            "MAC_X86_SHA256": mac_x86["sha256"],
            "LINUX_ARM_URL": linux_arm["url"],
            "LINUX_ARM_SHA256": linux_arm["sha256"],
            "LINUX_X86_URL": linux_x86["url"],
            "LINUX_X86_SHA256": linux_x86["sha256"],
        }
        template = TEMPLATES / "homebrew" / f"{name}.rb"
        out = OUTPUT / "homebrew" / f"{name}.rb"
        _render(template, out, mapping)


def _render_scoop(artifacts: list[dict], version: str) -> None:
    for name in ("molt", "molt-worker"):
        win = _find(artifacts, name, "windows", "x86_64")
        mapping = {
            "VERSION": version,
            "WIN_URL": win["url"],
            "WIN_SHA256": win["sha256"],
        }
        template = TEMPLATES / "scoop" / f"{name}.json"
        out = OUTPUT / "scoop" / f"{name}.json"
        _render(template, out, mapping)


def _render_winget(artifacts: list[dict], version: str) -> None:
    for name, winget_name in (("molt", "molt"), ("molt-worker", "molt-worker")):
        win = _find(artifacts, name, "windows", "x86_64")
        mapping = {
            "VERSION": version,
            "WIN_URL": win["url"],
            "WIN_SHA256": win["sha256"],
        }
        for suffix in (".yaml", ".installer.yaml", ".locale.en-US.yaml"):
            template = TEMPLATES / "winget" / f"{winget_name}{suffix}"
            out = OUTPUT / "winget" / f"{winget_name}{suffix}"
            _render(template, out, mapping)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest")
    args = parser.parse_args()

    data = _load_manifest(Path(args.manifest))
    version = data["version"]
    artifacts = data["artifacts"]

    _render_homebrew(artifacts, version)
    _render_scoop(artifacts, version)
    _render_winget(artifacts, version)


if __name__ == "__main__":
    main()
