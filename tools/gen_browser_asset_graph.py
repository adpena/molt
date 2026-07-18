#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import tomllib
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from molt.browser_asset_closure import (
    canonical_text_bytes,
    canonical_wasm_loader_asset_bytes,
)


ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "src" / "molt" / "browser_asset_graph.toml"
WASM_ROOT = ROOT / "wasm"
OUTPUT = WASM_ROOT / "browser_asset_graph.generated.json"
SCANNER_ROOT = ROOT / "tools" / "browser_asset_graph"
SCANNER = SCANNER_ROOT / "scan.mjs"
_ROLES = frozenset({"browser", "node", "shared"})
_SOURCE_TYPES = frozenset({"data", "module", "script"})


@dataclass(frozen=True, slots=True)
class AssetSource:
    path: str
    role: str
    source_type: str
    authority: str
    content: bytes


def _asset_name(value: str) -> str:
    path = PurePosixPath(value)
    if path.is_absolute() or not path.parts or ".." in path.parts:
        raise ValueError(f"browser asset path escapes the wasm root: {value!r}")
    normalized = path.as_posix()
    if normalized != value or normalized.startswith("./"):
        raise ValueError(f"browser asset path is not canonical: {value!r}")
    return normalized


def _read_manifest(
    manifest_path: Path = MANIFEST,
    wasm_root: Path = WASM_ROOT,
) -> tuple[dict[str, dict[str, object]], dict[str, AssetSource]]:
    try:
        manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(
            f"cannot read browser asset source manifest {manifest_path}: {exc}"
        ) from exc
    if manifest.get("schema_version") != 2:
        raise ValueError("unsupported browser asset source manifest schema")
    raw_rows = manifest.get("asset")
    if not isinstance(raw_rows, list):
        raise ValueError("browser asset source manifest has no asset rows")
    assets: dict[str, AssetSource] = {}
    for row in raw_rows:
        if not isinstance(row, dict):
            raise ValueError(
                "browser asset source manifest contains a malformed asset row"
            )
        raw_path = row.get("path")
        role = row.get("role")
        source_type = row.get("source_type")
        authority = row.get("authority")
        if (
            not isinstance(raw_path, str)
            or role not in _ROLES
            or source_type not in _SOURCE_TYPES
            or not isinstance(authority, str)
            or not authority
        ):
            raise ValueError(
                "browser asset source manifest contains a malformed asset row"
            )
        name = _asset_name(raw_path)
        if name in assets:
            raise ValueError(f"browser asset source manifest duplicates {name}")
        path = wasm_root.joinpath(*PurePosixPath(name).parts)
        try:
            content = canonical_wasm_loader_asset_bytes(path)
        except OSError as exc:
            raise FileNotFoundError(
                f"declared browser asset is missing: {path}"
            ) from exc
        if source_type == "data" and path.suffix not in {".json", ".wasm"}:
            raise ValueError(
                f"browser asset data row has an unsupported suffix: {name}"
            )
        if source_type != "data" and path.suffix not in {".js", ".mjs"}:
            raise ValueError(f"browser asset source row is not JavaScript: {name}")
        assets[name] = AssetSource(name, role, source_type, authority, content)

    raw_groups = manifest.get("entry_groups")
    if not isinstance(raw_groups, dict) or not raw_groups:
        raise ValueError("browser asset source manifest has no entry groups")
    groups: dict[str, dict[str, object]] = {}
    for name, row in raw_groups.items():
        if not isinstance(name, str) or not isinstance(row, dict):
            raise ValueError(
                "browser asset source manifest contains a malformed entry group"
            )
        role = row.get("role")
        entries = row.get("assets")
        if (
            role not in {"browser", "node"}
            or not isinstance(entries, list)
            or not entries
        ):
            raise ValueError(f"browser asset entry group is malformed: {name}")
        if not all(isinstance(entry, str) for entry in entries):
            raise ValueError(f"browser asset entry group is malformed: {name}")
        canonical_entries = [_asset_name(entry) for entry in entries]
        missing = sorted(set(canonical_entries) - set(assets))
        if missing:
            raise ValueError(
                f"browser asset entry group {name} names undeclared asset {missing[0]}"
            )
        for entry in canonical_entries:
            entry_role = assets[entry].role
            if entry_role not in {role, "shared"}:
                raise ValueError(
                    f"browser asset entry group {name} ({role}) cannot own {entry} ({entry_role})"
                )
        groups[name] = {"role": role, "assets": canonical_entries}
    return groups, assets


def _scanner_payload(sources: Iterable[AssetSource]) -> bytes:
    rows = [
        {
            "id": source.path,
            "path": source.path,
            "role": source.role,
            "source": source.content.decode("utf-8"),
            "source_type": source.source_type,
        }
        for source in sources
        if source.source_type != "data"
    ]
    return json.dumps({"sources": rows}, separators=(",", ":")).encode("utf-8")


def scan_sources(
    sources: Iterable[AssetSource],
) -> tuple[dict[str, list[dict[str, object]]], dict[str, int]]:
    """Scan all JavaScript sources in one checked Node process."""

    node = shutil.which("node")
    if node is None:
        raise RuntimeError("node is required to validate the browser ECMAScript graph")
    acorn_package = SCANNER_ROOT / "node_modules" / "acorn" / "package.json"
    if not acorn_package.is_file():
        raise RuntimeError(
            "Acorn parser is missing; run: python tools/bootstrap_browser_asset_graph.py"
        )
    if not SCANNER.is_file():
        raise RuntimeError(f"browser asset scanner is missing: {SCANNER}")
    result = subprocess.run(
        [node, str(SCANNER)],
        input=_scanner_payload(sources),
        capture_output=True,
        check=False,
        cwd=SCANNER_ROOT,
    )
    if result.returncode != 0:
        message = result.stderr.decode("utf-8", errors="replace").strip()
        raise ValueError(f"ECMAScript scanner rejected browser asset graph: {message}")
    try:
        payload = json.loads(result.stdout)
    except ValueError as exc:
        raise ValueError("ECMAScript scanner returned malformed JSON") from exc
    raw_results = payload.get("results")
    telemetry = payload.get("telemetry")
    if not isinstance(raw_results, list) or not isinstance(telemetry, dict):
        raise ValueError("ECMAScript scanner returned malformed results")
    by_path: dict[str, list[dict[str, object]]] = {}
    for row in raw_results:
        if not isinstance(row, dict) or not isinstance(row.get("path"), str):
            raise ValueError("ECMAScript scanner returned a malformed source result")
        references = row.get("references")
        if not isinstance(references, list) or not all(
            isinstance(reference, dict) for reference in references
        ):
            raise ValueError("ECMAScript scanner returned malformed references")
        by_path[row["path"]] = references
    numeric_telemetry = {
        key: value
        for key, value in telemetry.items()
        if isinstance(key, str) and isinstance(value, int) and value >= 0
    }
    return by_path, numeric_telemetry


def _resolve(owner: str, reference: str) -> str:
    parts = list(PurePosixPath(owner).parent.parts)
    for part in PurePosixPath(reference).parts:
        if part in {"", "."}:
            continue
        if part == "..":
            if not parts:
                raise ValueError(
                    f"browser asset reference escapes root: {owner} -> {reference}"
                )
            parts.pop()
        else:
            parts.append(part)
    return PurePosixPath(*parts).as_posix()


def _classify_reference(owner: AssetSource, reference: str) -> tuple[str, str]:
    if reference.startswith(("./", "../")):
        return "internal", _resolve(owner.path, reference)
    if reference.startswith("/"):
        raise ValueError(
            f"{owner.path} contains unsupported root-relative reference {reference!r}"
        )
    scheme = reference.partition(":")[0] if ":" in reference.partition("/")[0] else ""
    if scheme and scheme != "node":
        raise ValueError(
            f"{owner.path} contains unsupported URL reference {reference!r}"
        )
    if owner.role != "node":
        raise ValueError(
            f"{owner.path} ({owner.role}) contains bare module reference {reference!r}; "
            "bare dependencies are node-only"
        )
    return "external", reference


def _role_allows(owner: str, target: str) -> bool:
    if owner == "browser":
        return target in {"browser", "shared"}
    if owner == "node":
        return target in {"node", "shared"}
    return target == "shared"


def _reachable_assets(
    groups: Mapping[str, Mapping[str, object]],
    graph: Mapping[str, tuple[str, ...]],
) -> set[str]:
    reached: set[str] = set()
    pending = [
        entry
        for group in groups.values()
        for entry in group["assets"]
        if isinstance(entry, str)
    ]
    while pending:
        current = pending.pop()
        if current in reached:
            continue
        reached.add(current)
        pending.extend(graph[current])
    return reached


def generate_with_telemetry(
    manifest_path: Path = MANIFEST,
    wasm_root: Path = WASM_ROOT,
) -> tuple[bytes, dict[str, int]]:
    groups, assets = _read_manifest(manifest_path, wasm_root)
    scanned, telemetry = scan_sources(assets.values())
    generated_assets: dict[str, dict[str, object]] = {}
    graph: dict[str, tuple[str, ...]] = {}
    for name, source in assets.items():
        internal: set[str] = set()
        external: set[str] = set()
        facts: list[dict[str, object]] = []
        for raw_fact in scanned.get(name, []):
            request = raw_fact.get("request")
            kind = raw_fact.get("kind")
            line = raw_fact.get("line")
            column = raw_fact.get("column")
            if (
                not isinstance(request, str)
                or not isinstance(kind, str)
                or not isinstance(line, int)
                or not isinstance(column, int)
            ):
                raise ValueError(
                    f"ECMAScript scanner returned a malformed reference for {name}"
                )
            category, resolved = _classify_reference(source, request)
            fact = {
                "column": column,
                "kind": kind,
                "line": line,
                "request": request,
                "target": resolved,
            }
            facts.append(fact)
            if category == "internal":
                target = assets.get(resolved)
                if target is None:
                    raise ValueError(
                        f"{name} references undeclared browser asset {resolved}"
                    )
                if not _role_allows(source.role, target.role):
                    raise ValueError(
                        f"browser asset role violation: {name} ({source.role}) -> "
                        f"{resolved} ({target.role})"
                    )
                internal.add(resolved)
            else:
                external.add(resolved)
        references = tuple(sorted(internal))
        graph[name] = references
        generated_assets[name] = {
            "authority": source.authority,
            "external_references": sorted(external),
            "reference_facts": facts,
            "references": list(references),
            "role": source.role,
            "sha256": hashlib.sha256(source.content).hexdigest(),
            "source_type": source.source_type,
        }
    unreachable = sorted(set(assets) - _reachable_assets(groups, graph))
    if unreachable:
        raise ValueError(
            "browser asset inventory contains no-entry authority: "
            + ", ".join(unreachable)
        )
    payload: dict[str, Any] = {
        "assets": dict(sorted(generated_assets.items())),
        "entry_groups": dict(sorted(groups.items())),
        "schema_version": 2,
        "scanner": {
            "package_lock_sha256": hashlib.sha256(
                canonical_text_bytes(SCANNER_ROOT / "package-lock.json")
            ).hexdigest(),
            "path": SCANNER.relative_to(ROOT).as_posix(),
            "sha256": hashlib.sha256(canonical_text_bytes(SCANNER)).hexdigest(),
        },
    }
    return (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode(), telemetry


def generate(
    manifest_path: Path = MANIFEST,
    wasm_root: Path = WASM_ROOT,
) -> bytes:
    generated, _telemetry = generate_with_telemetry(manifest_path, wasm_root)
    return generated


def generated_output_is_current(output: Path, generated: bytes) -> bool:
    if not output.is_file():
        return False
    actual = output.read_bytes()
    return actual.replace(b"\r\n", b"\n").replace(b"\r", b"\n") == generated


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    generated = generate()
    if args.check:
        if generated_output_is_current(OUTPUT, generated):
            return 0
        print(
            f"browser asset graph is stale: {OUTPUT}; "
            "run tools/gen_browser_asset_graph.py",
            file=sys.stderr,
        )
        return 1
    OUTPUT.write_bytes(generated)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
