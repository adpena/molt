#!/usr/bin/env python3
"""Ratchet Rust dead-code masks and permanently cfg-disabled corpses."""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
REGISTRY = Path(__file__).resolve().parent / "dead_code_allow_baseline.json"
SCAN_ROOTS = ("runtime", "src")
ALLOW_RE = re.compile(r"#!?\[\s*allow\s*\(([^)]*)\)\s*\]")
CFG_CORPSE_RE = re.compile(
    r"#\[\s*cfg\s*\(\s*(?:any\s*\(\s*\)|not\s*\(\s*all\s*\(\s*\)\s*\))\s*\)\s*\]"
)
PLACEHOLDER = {"todo", "fixme", "later", "temporary", "tbd", "wip", "none"}


@dataclass(frozen=True)
class Site:
    id: str
    path: str
    line: int
    kind: str


def _valid_text(value: object) -> bool:
    text = str(value or "").strip()
    return len(text) >= 4 and text.lower() not in PLACEHOLDER


def scan(root: Path = ROOT) -> list[Site]:
    sites: list[Site] = []
    ordinals: dict[tuple[str, str], int] = {}
    for root_name in SCAN_ROOTS:
        source_root = root / root_name
        if not source_root.is_dir():
            continue
        for source in sorted(source_root.rglob("*.rs")):
            if "target" in source.parts or ".git" in source.parts:
                continue
            text = source.read_text(encoding="utf-8", errors="replace")
            rel = source.relative_to(root).as_posix()
            matches: list[tuple[int, str]] = []
            for match in ALLOW_RE.finditer(text):
                lints = {lint.strip() for lint in match.group(1).split(",")}
                if "dead_code" in lints:
                    matches.append((match.start(), "allow_dead_code"))
            matches.extend(
                (match.start(), "cfg_corpse") for match in CFG_CORPSE_RE.finditer(text)
            )
            for offset, kind in sorted(matches):
                key = (rel, kind)
                ordinal = ordinals.get(key, 0) + 1
                ordinals[key] = ordinal
                sites.append(
                    Site(
                        id=f"{rel}::{kind}::{ordinal}",
                        path=rel,
                        line=text.count("\n", 0, offset) + 1,
                        kind=kind,
                    )
                )
    return sites


def _load_registry(path: Path = REGISTRY) -> dict[str, object]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data.get("entries"), list):
        raise ValueError("registry must contain an entries list")
    return data


def regressions(sites: list[Site], registry: dict[str, object]) -> list[str]:
    failures: list[str] = []
    entries = registry["entries"]
    assert isinstance(entries, list)
    registered: dict[str, dict[str, object]] = {}
    for raw in entries:
        if not isinstance(raw, dict) or not isinstance(raw.get("id"), str):
            failures.append("invalid registry entry")
            continue
        site_id = raw["id"]
        if site_id in registered:
            failures.append(f"duplicate registry entry: {site_id}")
            continue
        registered[site_id] = raw
        if not _valid_text(raw.get("owner")):
            failures.append(f"missing owner: {site_id}")
        if not _valid_text(raw.get("waiver")):
            failures.append(f"missing waiver rationale: {site_id}")
    live = {site.id: site for site in sites}
    for site_id, site in live.items():
        if site_id not in registered:
            failures.append(
                f"unwaived {site.kind}: {site.path}:{site.line} ({site.id})"
            )
    for site_id in sorted(set(registered) - set(live)):
        failures.append(f"stale registry entry must be removed: {site_id}")
    baseline_total = int(registry.get("baseline_total", len(entries)))
    if len(entries) > baseline_total:
        failures.append(
            f"ratchet regression: {len(entries)} entries exceed baseline {baseline_total}"
        )
    return failures


def _write_registry(sites: list[Site], owner: str, path: Path = REGISTRY) -> None:
    entries = [
        {
            **asdict(site),
            "owner": owner,
            "waiver": "legacy dead-code mask; owner must wire or delete",
        }
        for site in sites
    ]
    path.write_bytes(
        (
            json.dumps({"baseline_total": len(entries), "entries": entries}, indent=2)
            + "\n"
        ).encode("utf-8")
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--update", action="store_true")
    parser.add_argument("--owner", default="compiler-runtime maintainers")
    args = parser.parse_args(argv)
    sites = scan()
    if args.update:
        if not _valid_text(args.owner):
            print(
                "dead_code_allow_ratchet: --owner must name a real owner",
                file=sys.stderr,
            )
            return 3
        _write_registry(sites, args.owner)
        print(f"dead_code_allow_ratchet: registry updated to {len(sites)} sites")
        return 0
    try:
        registry = _load_registry()
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"dead_code_allow_ratchet: invalid registry: {exc}", file=sys.stderr)
        return 3
    failures = regressions(sites, registry)
    if failures:
        print("dead_code_allow_ratchet: FAIL", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 2
    print(
        f"dead_code_allow_ratchet: PASS - {len(sites)} registered sites <= "
        f"baseline {registry['baseline_total']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
