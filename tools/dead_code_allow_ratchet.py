#!/usr/bin/env python3
"""Ratchet Rust dead-code masks and permanently cfg-disabled corpses."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
# The waiver registry, relative to the scanned checkout.
REGISTRY_RELPATH = Path("tools") / "dead_code_allow_baseline.json"
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


class UnnamedSiteError(ValueError):
    """A mask sits before an item shape the scanner cannot name."""


_ITEM_RE = re.compile(
    r"""
    (?:pub(?:\s*\([^)]*\))?\s+)?
    (?:(?:default|unsafe|safe|async|extern(?:\s+"[^"]*")?
        |const(?=\s+(?:unsafe|async|extern|fn)\b))\s+)*
    (?P<kind>(?:fn|struct|enum|union|trait|type|mod|static|const|impl|use|let)\b
        |macro_rules!)
    """,
    re.VERBOSE,
)
_NAME_RE = re.compile(r"\s*(?:mut\s+)?(?P<name>[A-Za-z_][A-Za-z0-9_]*)")
_MEMBER_RE = re.compile(
    r"(?:pub(?:\s*\([^)]*\))?\s+)?(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*(?P<sep>::|[:({,=])"
)


def _skip_string(text: str, pos: int) -> int:
    """Return the offset after the string literal that starts at ``pos``."""
    pos += 1
    while pos < len(text):
        if text[pos] == "\\":
            pos += 2
        elif text[pos] == '"':
            return pos + 1
        else:
            pos += 1
    return pos


def _skip_attribute(text: str, pos: int) -> int:
    """Return the offset after the ``#[...]`` or ``#![...]`` at ``pos``."""
    pos = text.index("[", pos)
    depth = 0
    while pos < len(text):
        char = text[pos]
        if char == '"':
            pos = _skip_string(text, pos)
            continue
        if char == "[":
            depth += 1
        elif char == "]":
            depth -= 1
            if depth == 0:
                return pos + 1
        pos += 1
    return pos


def _skip_trivia(text: str, pos: int) -> int:
    """Skip whitespace, comments and further attributes before an item."""
    while pos < len(text):
        if text[pos].isspace():
            pos += 1
        elif text.startswith("//", pos):
            newline = text.find("\n", pos)
            pos = len(text) if newline < 0 else newline + 1
        elif text.startswith("/*", pos):
            depth, pos = 1, pos + 2
            while pos < len(text) and depth:
                if text.startswith("/*", pos):
                    depth, pos = depth + 1, pos + 2
                elif text.startswith("*/", pos):
                    depth, pos = depth - 1, pos + 2
                else:
                    pos += 1
        elif text.startswith("#[", pos) or text.startswith("#![", pos):
            pos = _skip_attribute(text, pos)
        else:
            return pos
    return pos


_RAW_STRING_RE = re.compile(r'b?r(#*)"')
_FN_RE = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")


def _literal_spans(text: str) -> tuple[list[tuple[int, int]], list[tuple[int, int]]]:
    """Return the comment spans and the string-literal spans of Rust source."""
    comments: list[tuple[int, int]] = []
    strings: list[tuple[int, int]] = []
    pos = 0
    while pos < len(text):
        char = text[pos]
        if text.startswith("//", pos):
            newline = text.find("\n", pos)
            end = len(text) if newline < 0 else newline
            comments.append((pos, end))
            pos = end
        elif text.startswith("/*", pos):
            start, depth, pos = pos, 1, pos + 2
            while pos < len(text) and depth:
                if text.startswith("/*", pos):
                    depth, pos = depth + 1, pos + 2
                elif text.startswith("*/", pos):
                    depth, pos = depth - 1, pos + 2
                else:
                    pos += 1
            comments.append((start, pos))
        elif (raw := _RAW_STRING_RE.match(text, pos)) is not None and (
            pos == 0 or not (text[pos - 1].isalnum() or text[pos - 1] == "_")
        ):
            closing = '"' + raw.group(1)
            end = text.find(closing, raw.end())
            end = len(text) if end < 0 else end + len(closing)
            strings.append((pos, end))
            pos = end
        elif char == '"':
            end = _skip_string(text, pos)
            strings.append((pos, end))
            pos = end
        elif char == "'":
            # A char literal closes within a few characters; a lifetime does not.
            if text.startswith("\\", pos + 1):
                close = text.find("'", pos + 2)
                pos = close + 1 if close >= 0 else pos + 1
            elif pos + 2 < len(text) and text[pos + 2] == "'":
                pos += 3
            else:
                pos += 1
        else:
            pos += 1
    return comments, strings


def _within(spans: list[tuple[int, int]], offset: int) -> bool:
    return any(start <= offset < end for start, end in spans)


def _enclosing_fn(text: str, offset: int, strings: list[tuple[int, int]]) -> str:
    names = [
        match.group(1)
        for match in _FN_RE.finditer(text, 0, offset)
        if not _within(strings, match.start())
    ]
    if not names:
        line = text.count("\n", 0, offset) + 1
        raise UnnamedSiteError(f"line {line}: emitted mask outside any fn")
    return names[-1]


def _masked_item(text: str, attribute_start: int) -> str:
    """Name the item an attribute masks, for a site ID that survives edits.

    An inner attribute masks its enclosing scope. Otherwise the item is the
    next declaration after any comments and attributes: ``fn:name``,
    ``impl:<header>``, ``field:name`` and so on.
    """
    if text.startswith("#!", attribute_start):
        return "inner"
    pos = _skip_trivia(text, _skip_attribute(text, attribute_start))
    item = _ITEM_RE.match(text, pos)
    if item is not None:
        kind = item.group("kind").rstrip("!")
        rest = item.end()
        if kind == "impl":
            header = re.split(r"\{|\bwhere\b", text[rest : rest + 400], maxsplit=1)[0]
            return "impl:" + " ".join(header.split())
        if kind == "use":
            path = text[rest : text.find(";", rest)]
            return "use:" + "".join(path.split())
        name = _NAME_RE.match(text, rest)
        if name is not None:
            return f"{kind}:{name.group('name')}"
    member = _MEMBER_RE.match(text, pos)
    if member is not None and member.group("sep") != "::":
        kind = "field" if member.group("sep") == ":" else "variant"
        return f"{kind}:{member.group('name')}"
    line = text.count("\n", 0, attribute_start) + 1
    snippet = text[pos : pos + 60].split("\n", 1)[0]
    raise UnnamedSiteError(f"line {line}: cannot name the masked item at {snippet!r}")


def scan(root: Path = ROOT) -> list[Site]:
    sites: list[Site] = []
    for root_name in SCAN_ROOTS:
        source_root = root / root_name
        if not source_root.is_dir():
            continue
        for source in sorted(source_root.rglob("*.rs")):
            rel = source.relative_to(root).as_posix()
            if {"target", ".git"} & set(Path(rel).parts):
                continue
            text = source.read_text(encoding="utf-8", errors="replace")
            comments, strings = _literal_spans(text)
            matches: list[tuple[int, str]] = []
            for match in ALLOW_RE.finditer(text):
                lints = {lint.strip() for lint in match.group(1).split(",")}
                if "dead_code" in lints:
                    matches.append((match.start(), "allow_dead_code"))
            matches.extend(
                (match.start(), "cfg_corpse") for match in CFG_CORPSE_RE.finditer(text)
            )
            occurrences: dict[str, int] = {}
            for offset, kind in sorted(matches):
                if _within(comments, offset):
                    continue
                try:
                    # A generator's string literal emits the mask into generated
                    # code; the generating function names that site.
                    item = (
                        f"emitted-by:{_enclosing_fn(text, offset, strings)}"
                        if _within(strings, offset)
                        else _masked_item(text, offset)
                    )
                except UnnamedSiteError as exc:
                    raise UnnamedSiteError(f"{rel}: {exc}") from None
                base = f"{rel}::{kind}::{item}"
                occurrence = occurrences.get(base, 0) + 1
                occurrences[base] = occurrence
                sites.append(
                    Site(
                        id=base if occurrence == 1 else f"{base}#{occurrence}",
                        path=rel,
                        line=text.count("\n", 0, offset) + 1,
                        kind=kind,
                    )
                )
    return sites


def _load_registry(root: Path) -> dict[str, object]:
    data = json.loads((root / REGISTRY_RELPATH).read_text(encoding="utf-8"))
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


def updated_entries(
    sites: list[Site],
    registry: dict[str, object] | None,
    *,
    owner: str | None,
    waiver: str | None,
) -> list[dict[str, str]]:
    """Keep each live site's waiver, drop stale ones, and waive new sites.

    A new site takes the given owner and waiver, which must both be real; no
    default waiver exists, so a new mask is a reviewed registry change.
    """
    registered = {
        entry["id"]: entry
        for entry in (registry or {}).get("entries", [])
        if isinstance(entry, dict) and isinstance(entry.get("id"), str)
    }
    entries: list[dict[str, str]] = []
    for site in sites:
        prior = registered.get(site.id)
        if prior is not None:
            entries.append(
                {"id": site.id, "owner": prior["owner"], "waiver": prior["waiver"]}
            )
        elif _valid_text(owner) and _valid_text(waiver):
            assert owner is not None and waiver is not None
            entries.append({"id": site.id, "owner": owner, "waiver": waiver})
        else:
            raise ValueError(
                f"new site {site.id} needs --owner and --waiver naming why it stays"
            )
    return entries


def _write_registry(entries: list[dict[str, str]], root: Path) -> None:
    (root / REGISTRY_RELPATH).write_bytes(
        (
            json.dumps({"baseline_total": len(entries), "entries": entries}, indent=2)
            + "\n"
        ).encode("utf-8")
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--update",
        action="store_true",
        help="rewrite the registry: keep live waivers, drop stale entries and "
        "waive new sites with --owner and --waiver",
    )
    parser.add_argument("--owner", help="owner of the new sites --update waives")
    parser.add_argument("--waiver", help="why the new sites --update waives stay")
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="checkout to scan; its tools/dead_code_allow_baseline.json is the "
        "registry (default: this repository)",
    )
    args = parser.parse_args(argv)
    root = args.root.resolve()
    try:
        sites = scan(root)
    except UnnamedSiteError as exc:
        print(f"dead_code_allow_ratchet: {exc}", file=sys.stderr)
        return 3
    try:
        registry = _load_registry(root)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        if not args.update:
            print(f"dead_code_allow_ratchet: invalid registry: {exc}", file=sys.stderr)
            return 3
        registry = None
    if args.update:
        try:
            entries = updated_entries(
                sites, registry, owner=args.owner, waiver=args.waiver
            )
        except ValueError as exc:
            print(f"dead_code_allow_ratchet: {exc}", file=sys.stderr)
            return 3
        _write_registry(entries, root)
        print(f"dead_code_allow_ratchet: registry updated to {len(entries)} sites")
        return 0
    assert registry is not None
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
