"""Hash-bound canonical asset graph for Molt's browser and Node WASM loaders."""

from __future__ import annotations

import hashlib
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from molt.source_root import compiler_source_root
from molt.exact_json import read_exact
from molt.toolchain_identity import capture_stable_regular_file


BROWSER_WASM_ENTRY_ASSETS = "browser-wasm"
BROWSER_HOST_ENTRY_ASSETS = "browser-host"
NODE_RUNNER_ENTRY_ASSETS = "node-runner"
_GRAPH_NAME = "browser_asset_graph.generated.json"
_CANONICAL_TEXT_SUFFIXES = frozenset({".js", ".json", ".mjs"})
_MAX_ASSET_BYTES = 16 * 1024 * 1024
_MAX_GRAPH_BYTES = 4 * 1024 * 1024
_MAX_CLOSURE_BYTES = 64 * 1024 * 1024


def canonical_text_bytes(path: Path) -> bytes:
    """Read UTF-8 text with one LF wire representation on every host."""

    _, raw = capture_stable_regular_file(
        path, label="WASM loader text", max_bytes=_MAX_ASSET_BYTES
    )
    text = raw.decode("utf-8")
    if "\r" not in text:
        return raw
    return text.replace("\r\n", "\n").replace("\r", "\n").encode("utf-8")


def canonical_wasm_loader_asset_bytes(path: Path) -> bytes:
    """Read a loader asset in its deterministic publication wire form."""

    if path.suffix not in _CANONICAL_TEXT_SUFFIXES:
        return capture_stable_regular_file(
            path, label="WASM loader asset", max_bytes=_MAX_ASSET_BYTES
        )[1]
    return canonical_text_bytes(path)


@dataclass(frozen=True, slots=True)
class _Asset:
    references: tuple[str, ...]
    role: str
    sha256: object


def _canonical_asset_path(root: Path, name: str) -> Path:
    relative = PurePosixPath(name)
    if relative.is_absolute() or not relative.parts or ".." in relative.parts:
        raise ValueError(f"browser asset graph path escapes the wasm root: {name}")
    path = root.joinpath(*relative.parts).resolve()
    if not path.is_relative_to(root):
        raise ValueError(f"browser asset graph path escapes the wasm root: {name}")
    return path


def _verified_closure(
    wasm_root: Path,
    entries: str | Iterable[str],
    *,
    retain_payloads: bool,
) -> tuple[tuple[str, ...], dict[str, bytes]]:
    root = wasm_root.resolve()
    graph_path = root / _GRAPH_NAME
    try:
        payload = read_exact(
            graph_path, max_bytes=_MAX_GRAPH_BYTES, label="browser asset graph"
        )
    except (OSError, ValueError) as exc:
        raise ValueError(
            f"browser asset graph is unreadable: {graph_path}: {exc}"
        ) from exc
    if (
        not isinstance(payload, dict)
        or payload.get("schema_version") != 2
        or not isinstance(payload.get("assets"), dict)
    ):
        raise ValueError(f"browser asset graph has unsupported schema: {graph_path}")
    graph: dict[str, _Asset] = {}
    for name, facts in payload["assets"].items():
        if not isinstance(name, str) or not isinstance(facts, dict):
            raise ValueError("browser asset graph contains a malformed asset row")
        role = facts.get("role")
        references = facts.get("references")
        if (
            role not in {"browser", "node", "shared"}
            or not isinstance(references, list)
            or not all(isinstance(reference, str) for reference in references)
        ):
            raise ValueError(f"browser asset graph references are malformed for {name}")
        graph[name] = _Asset(tuple(references), role, facts.get("sha256"))
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
        group_assets = row.get("assets")
        if (
            role not in {"browser", "node"}
            or not isinstance(group_assets, list)
            or not all(isinstance(entry, str) for entry in group_assets)
        ):
            raise ValueError("browser asset graph contains a malformed entry group")
        missing = sorted(set(group_assets) - set(graph))
        if missing:
            raise ValueError(
                f"browser asset graph entry group {name} names undeclared asset {missing[0]}"
            )
        groups[name] = role, tuple(group_assets)
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
    names = tuple(sorted(seen))
    retained: dict[str, bytes] = {}
    total_bytes = 0
    # Validate every declared asset even when its bytes are not requested.
    # The names projection has the same global drift/error boundary, but does
    # not retain payloads. Staging keeps only its selected immutable captures.
    for name, facts in graph.items():
        path = _canonical_asset_path(root, name)
        if not path.is_file():
            raise FileNotFoundError(f"missing browser static asset: {path}")
        data = canonical_wasm_loader_asset_bytes(path)
        total_bytes += len(data)
        if total_bytes > _MAX_CLOSURE_BYTES:
            raise ValueError("browser asset graph exceeds payload byte limit")
        actual_hash = hashlib.sha256(data).hexdigest()
        if facts.sha256 != actual_hash:
            raise ValueError(
                f"browser asset graph hash drift for {name}: "
                f"expected {facts.sha256}, got {actual_hash}; run tools/gen_browser_asset_graph.py --write"
            )
        if retain_payloads and name in seen:
            retained[name] = data
        del data
    return names, {name: retained[name] for name in names} if retain_payloads else {}


def wasm_loader_asset_payloads(
    wasm_root: Path,
    entries: str | Iterable[str] = BROWSER_WASM_ENTRY_ASSETS,
) -> dict[str, bytes]:
    """Retain only requested bytes, while verifying the entire generated graph."""
    return _verified_closure(wasm_root, entries, retain_payloads=True)[1]


def wasm_loader_asset_closure(
    wasm_root: Path,
    entries: str | Iterable[str] = BROWSER_WASM_ENTRY_ASSETS,
) -> tuple[str, ...]:
    """Project names for dependency scopes and direct source discovery."""
    return _verified_closure(wasm_root, entries, retain_payloads=False)[0]


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
    entries: str | Iterable[str] = BROWSER_WASM_ENTRY_ASSETS,
) -> tuple[str, ...]:
    """Return proof scopes from source authority, never a staged witness root."""

    return tuple(
        f"wasm/{asset}"
        for asset in wasm_loader_asset_closure(compiler_source_root() / "wasm", entries)
    )
