"""Hash-bound canonical asset graph for Molt's browser and Node WASM loaders."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path, PurePosixPath


BROWSER_WASM_ENTRY_ASSETS = "browser-wasm"
BROWSER_HOST_ENTRY_ASSETS = "browser-host"
NODE_RUNNER_ENTRY_ASSETS = "node-runner"
_GRAPH_NAME = "browser_asset_graph.generated.json"


@dataclass(frozen=True, slots=True)
class _VerifiedAsset:
    references: tuple[str, ...]
    role: str


def _canonical_asset_path(root: Path, name: str) -> Path:
    relative = PurePosixPath(name)
    if relative.is_absolute() or not relative.parts or ".." in relative.parts:
        raise ValueError(f"browser asset graph path escapes the wasm root: {name}")
    path = root.joinpath(*relative.parts).resolve()
    if not path.is_relative_to(root):
        raise ValueError(f"browser asset graph path escapes the wasm root: {name}")
    return path


def _load_verified_graph(
    wasm_root: Path,
) -> tuple[dict[str, _VerifiedAsset], dict[str, tuple[str, tuple[str, ...]]]]:
    root = wasm_root.resolve()
    graph_path = root / _GRAPH_NAME
    try:
        graph_bytes = graph_path.read_bytes()
        payload = json.loads(graph_bytes)
    except (OSError, ValueError) as exc:
        raise ValueError(
            f"browser asset graph is unreadable: {graph_path}: {exc}"
        ) from exc
    if payload.get("schema_version") != 2 or not isinstance(
        payload.get("assets"), dict
    ):
        raise ValueError(f"browser asset graph has unsupported schema: {graph_path}")
    graph: dict[str, _VerifiedAsset] = {}
    for name, facts in payload["assets"].items():
        if not isinstance(name, str) or not isinstance(facts, dict):
            raise ValueError("browser asset graph contains a malformed asset row")
        path = _canonical_asset_path(root, name)
        if not path.is_file():
            raise FileNotFoundError(f"missing browser static asset: {path}")
        expected_hash = facts.get("sha256")
        actual_hash = hashlib.sha256(path.read_bytes()).hexdigest()
        if expected_hash != actual_hash:
            raise ValueError(
                f"browser asset graph hash drift for {name}: "
                f"expected {expected_hash}, got {actual_hash}; run tools/gen_browser_asset_graph.py"
            )
        role = facts.get("role")
        references = facts.get("references")
        if (
            role not in {"browser", "node", "shared"}
            or not isinstance(references, list)
            or not all(isinstance(reference, str) for reference in references)
        ):
            raise ValueError(f"browser asset graph references are malformed for {name}")
        graph[name] = _VerifiedAsset(tuple(references), role)
    for owner, facts in graph.items():
        missing = sorted(set(facts.references) - set(graph))
        if missing:
            raise ValueError(
                f"browser asset graph {owner} references undeclared asset {missing[0]}"
            )
        for reference in facts.references:
            target_role = graph[reference].role
            allowed = (
                target_role in {facts.role, "shared"}
                if facts.role != "shared"
                else target_role == "shared"
            )
            if not allowed:
                raise ValueError(
                    f"browser asset graph role violation: {owner} ({facts.role}) -> "
                    f"{reference} ({target_role})"
                )
    raw_groups = payload.get("entry_groups")
    if not isinstance(raw_groups, dict) or not raw_groups:
        raise ValueError("browser asset graph has no named entry groups")
    groups: dict[str, tuple[str, tuple[str, ...]]] = {}
    for name, row in raw_groups.items():
        if not isinstance(name, str) or not isinstance(row, dict):
            raise ValueError("browser asset graph contains a malformed entry group")
        role = row.get("role")
        entries = row.get("assets")
        if (
            role not in {"browser", "node"}
            or not isinstance(entries, list)
            or not all(isinstance(entry, str) for entry in entries)
        ):
            raise ValueError("browser asset graph contains a malformed entry group")
        missing = sorted(set(entries) - set(graph))
        if missing:
            raise ValueError(
                f"browser asset graph entry group {name} names undeclared asset {missing[0]}"
            )
        groups[name] = role, tuple(entries)
    return graph, groups


def wasm_loader_asset_closure(
    wasm_root: Path,
    entries: str | Iterable[str] = BROWSER_WASM_ENTRY_ASSETS,
) -> tuple[str, ...]:
    """Return a verified browser/Node loader closure from the generated graph."""

    graph, groups = _load_verified_graph(wasm_root)
    expected_role: str | None = None
    if isinstance(entries, str):
        if entries not in groups:
            raise ValueError(f"browser asset entry group is absent: {entries}")
        expected_role, group_entries = groups[entries]
        pending = list(group_entries)
    else:
        pending = list(entries)
    if not pending:
        raise ValueError("browser asset closure requires at least one entry")
    seen: set[str] = set()
    while pending:
        asset = pending.pop()
        if asset in seen:
            continue
        facts = graph.get(asset)
        if facts is None:
            raise ValueError(
                f"browser asset entry is absent from generated graph: {asset}"
            )
        if expected_role is not None and facts.role not in {expected_role, "shared"}:
            raise ValueError(
                f"browser asset entry group role drifted: {asset} is {facts.role}, "
                f"expected {expected_role} or shared"
            )
        seen.add(asset)
        pending.extend(facts.references)
    return tuple(sorted(seen))


def browser_asset_manifest_key(asset: str) -> str:
    stem = PurePosixPath(asset).name
    for suffix in (".generated.js", "_generated.js", ".mjs", ".js"):
        if stem.endswith(suffix):
            stem = stem[: -len(suffix)]
            break
    return stem.replace("-", "_")


def browser_asset_manifest_keys(assets: Iterable[str]) -> dict[str, str]:
    result: dict[str, str] = {}
    owners: dict[str, str] = {}
    for asset in assets:
        key = browser_asset_manifest_key(asset)
        previous = owners.get(key)
        if previous is not None and previous != asset:
            raise ValueError(
                f"browser assets {previous!r} and {asset!r} collide at manifest key {key!r}"
            )
        owners[key] = asset
        result[asset] = key
    return result


def wasm_loader_asset_scope_paths(
    repo_root: Path,
    entries: str | Iterable[str] = BROWSER_WASM_ENTRY_ASSETS,
) -> tuple[str, ...]:
    return tuple(
        f"wasm/{asset}"
        for asset in wasm_loader_asset_closure(repo_root / "wasm", entries)
    )
