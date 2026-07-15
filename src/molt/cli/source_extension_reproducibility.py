from __future__ import annotations

import hashlib
import json
import os
import re
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any


_MESON_TRANSIENT_DEPENDENCY_ID_RE = re.compile(r"dep[0-9]+")


def _ordered_location_roots(
    roots: Sequence[tuple[Path | None, str]],
) -> tuple[tuple[Path, str], ...]:
    """Return stable path authorities with descendants before ancestors.

    Independent roots retain their declared semantic order. Sorting every root by
    host-path length makes artifact identity depend on an arbitrary build-directory
    spelling; containment is the only ordering relation that path replacement needs.
    """
    deduped: list[tuple[Path, str]] = []
    seen: set[Path] = set()
    for path, replacement in roots:
        if path is None:
            continue
        resolved = path.resolve()
        if resolved in seen:
            continue
        seen.add(resolved)
        deduped.append((resolved, replacement))

    def ancestor_count(candidate: Path) -> int:
        return sum(
            candidate != other and candidate.is_relative_to(other)
            for other, _replacement in deduped
        )

    return tuple(
        item
        for _index, item in sorted(
            enumerate(deduped),
            key=lambda indexed: (
                -ancestor_count(indexed[1][0]),
                indexed[0],
            ),
        )
    )


def _source_extension_deterministic_path_args(
    *,
    compiler_command: Sequence[str],
    roots: Sequence[tuple[Path | None, str]],
) -> list[str]:
    """Map every build-location authority out of compiler-visible bytes."""
    if not compiler_command:
        return []
    ordered = _ordered_location_roots(roots)
    tool = Path(compiler_command[0]).name.lower()
    if tool in {"cl", "cl.exe", "clang-cl", "clang-cl.exe"}:
        return [f"/pathmap:{path}={replacement}" for path, replacement in ordered]
    return [
        argument
        for path, replacement in ordered
        for argument in (
            f"-ffile-prefix-map={path}={replacement}",
            f"-fdebug-prefix-map={path}={replacement}",
            f"-fmacro-prefix-map={path}={replacement}",
        )
    ]


def _canonicalize_location_string(
    value: str,
    location_roots: Sequence[tuple[Path | None, str]],
) -> str:
    return _canonicalize_location_string_ordered(
        value, _ordered_location_roots(location_roots)
    )


def _canonicalize_location_string_ordered(
    value: str,
    ordered_roots: Sequence[tuple[Path, str]],
) -> str:
    canonical = value.replace("\\", "/")
    for root, token in ordered_roots:
        root_text = root.as_posix().rstrip("/")
        if not root_text:
            raise ValueError("filesystem root cannot be a location identity authority")
        flags = re.IGNORECASE if root.drive else 0
        canonical = re.sub(
            re.escape(root_text) + r'''(?=$|[/=;,\s'"\)\]\}])''',
            lambda _match: token,
            canonical,
            flags=flags,
        )
    return canonical


def _canonicalize_locations(
    value: Any,
    location_roots: Sequence[tuple[Path | None, str]],
    source_paths: Mapping[Path, str] | None = None,
) -> Any:
    ordered_roots = _ordered_location_roots(location_roots)
    resolved_source_paths: dict[str, str] | None = None
    if source_paths is not None:
        resolved_source_paths = {}
        for path, replacement in source_paths.items():
            for candidate in (path.expanduser(), path.resolve()):
                key = os.path.normcase(os.path.normpath(os.fspath(candidate)))
                previous = resolved_source_paths.setdefault(key, replacement)
                if previous != replacement:
                    raise ValueError(
                        "source-path canonicalization has conflicting identities: "
                        f"{candidate} -> {previous!r} and {replacement!r}"
                    )

    def canonicalize(item: Any) -> Any:
        if isinstance(item, str):
            if resolved_source_paths is not None:
                expanded = os.path.expanduser(item)
                candidate = (
                    os.path.normcase(os.path.normpath(expanded))
                    if os.path.isabs(expanded)
                    else None
                )
            else:
                candidate = None
            if candidate is not None:
                replacement = resolved_source_paths.get(candidate)
                if replacement is not None:
                    return replacement
            return _canonicalize_location_string_ordered(item, ordered_roots)
        if isinstance(item, list):
            return [canonicalize(child) for child in item]
        if isinstance(item, dict):
            canonical: dict[str, Any] = {}
            for raw_key, child in item.items():
                key = _canonicalize_location_string_ordered(
                    str(raw_key), ordered_roots
                )
                if key in canonical:
                    raise ValueError(
                        "location canonicalization collapses distinct metadata keys: "
                        f"{raw_key!r} -> {key!r}"
                    )
                canonical[key] = canonicalize(child)
            return canonical
        return item

    return canonicalize(value)


def _canonicalize_meson_metadata(
    value: Any,
    location_roots: Sequence[tuple[Path | None, str]],
) -> Any:
    """Remove host locations and Meson's randomized dependency identities.

    Meson dependency IDs are process-local opaque handles. Their equality within
    one introspection document is useful provenance; their random decimal payload
    is not. Stable first-occurrence ordinals preserve that equality relation.
    """
    canonical = _canonicalize_locations(value, location_roots)
    dependency_ids: dict[str, str] = {}

    def replace_transient_ids(item: Any, *, in_dependencies: bool = False) -> Any:
        if isinstance(item, str):
            if (
                not in_dependencies
                or _MESON_TRANSIENT_DEPENDENCY_ID_RE.fullmatch(item) is None
            ):
                return item
            return dependency_ids.setdefault(
                item,
                f"@meson-dependency/{len(dependency_ids):04d}",
            )
        if isinstance(item, list):
            return [
                replace_transient_ids(child, in_dependencies=in_dependencies)
                for child in item
            ]
        if isinstance(item, dict):
            normalized: dict[str, Any] = {}
            for raw_key in sorted(item):
                normalized[raw_key] = replace_transient_ids(
                    item[raw_key],
                    in_dependencies=raw_key == "dependencies",
                )
            return normalized
        return item

    return replace_transient_ids(canonical)


def _canonical_json_sha256(
    path: Path,
    *,
    location_roots: Sequence[tuple[Path | None, str]],
    normalize_meson_dependency_ids: bool,
) -> str:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"cannot canonicalize JSON identity input {path}: {exc}") from exc
    canonical = (
        _canonicalize_meson_metadata(payload, location_roots)
        if normalize_meson_dependency_ids
        else _canonicalize_locations(payload, location_roots)
    )
    encoded = json.dumps(canonical, sort_keys=True, indent=2).encode("utf-8") + b"\n"
    return hashlib.sha256(encoded).hexdigest()


def _canonical_extension_manifest_for_wheel(
    manifest: Mapping[str, Any],
    *,
    location_roots: Sequence[tuple[Path | None, str]],
    meson_plan_path: Path | None = None,
    compile_commands_path: Path | None = None,
) -> dict[str, Any]:
    """Project an operational sidecar into a location-neutral wheel identity."""
    canonical = _canonicalize_locations(dict(manifest), location_roots)
    assert isinstance(canonical, dict)
    source_plan = canonical.get("source_plan")
    if isinstance(source_plan, dict):
        if meson_plan_path is not None:
            source_plan["plan_sha256"] = _canonical_json_sha256(
                meson_plan_path,
                location_roots=location_roots,
                normalize_meson_dependency_ids=True,
            )
        if compile_commands_path is not None:
            source_plan["compile_commands_sha256"] = _canonical_json_sha256(
                compile_commands_path,
                location_roots=location_roots,
                normalize_meson_dependency_ids=False,
            )
        plan_identity = dict(source_plan)
        plan_identity.pop("digest", None)
        source_plan["digest"] = hashlib.sha256(
            json.dumps(
                plan_identity,
                sort_keys=True,
                separators=(",", ":"),
            ).encode("utf-8")
        ).hexdigest()
        build = canonical.get("build")
        if isinstance(build, dict):
            build["source_plan_digest"] = source_plan["digest"]
    closure = canonical.get("object_closure")
    if isinstance(closure, dict) and "closure_sha256" in closure:
        closure_identity = dict(closure)
        closure_identity.pop("closure_sha256", None)
        closure["closure_sha256"] = hashlib.sha256(
            json.dumps(
                closure_identity,
                sort_keys=True,
                separators=(",", ":"),
            ).encode("utf-8")
        ).hexdigest()
        build = canonical.get("build")
        if isinstance(build, dict):
            build["object_closure_sha256"] = closure["closure_sha256"]
    return canonical
