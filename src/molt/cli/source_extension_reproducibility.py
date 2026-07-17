"""Location-neutral source-extension build and wheel metadata authority."""

from __future__ import annotations

import hashlib
import json
import os
import re
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

_MESON_TRANSIENT_DEPENDENCY_ID_RE = re.compile(r"dep[0-9]+")
_URL_SCHEME_RE = re.compile(r"[A-Za-z][A-Za-z0-9+.-]*://")
_FILE_URL_RE = re.compile(r"(?i)file://")
_WINDOWS_ABSOLUTE_RE = re.compile(r"(?i)(?<![A-Za-z0-9_])([A-Z]):/{1,}")
_UNC_ABSOLUTE_RE = re.compile(r"(?<!:)//[^/\s]+/[^/\s]+")
_POSIX_ABSOLUTE_RE = re.compile(
    r"(?:^|(?<=[=,:;\s'\"\(\[\{]))/(?!/)[^/\s'\"\)\]\}]+(?:/[^\s'\"\)\]\}]*)?"
)
_HOME_PATH_RE = re.compile(
    r"(?i)(?:^|(?<=[=,:;\s'\"\(\[\{]))(?:~(?:/|$)|\$HOME(?:/|$)|"
    r"\$\{HOME\}(?:/|$)|%(?:USERPROFILE|HOME)%(?:/|$))"
)
_JOINED_PATH_FLAG_RE = re.compile(
    r"(?i)(?:^|(?<=\s))(?:-I|-L|-isystem|-iquote|-include|--sysroot=|"
    r"/I|/LIBPATH:|@)(?P<path>(?:[A-Z]:/+|//|/)[^\s'\"]+)"
)
_JOINED_PATH_PREFIXES = (
    "-I",
    "-L",
    "-isystem",
    "-iquote",
    "-include",
    "--sysroot=",
    "/I",
    "/LIBPATH:",
    "@",
)
_MSVC_PATH_FLAG_PREFIXES = ("/I", "/LIBPATH:", "/Fo", "/Fd", "/Fe")


def _inside_url_token(value: str, index: int) -> bool:
    token_start = index
    while token_start and value[token_start - 1] not in " \t\r\n'\"()[]{}":
        token_start -= 1
    match = _URL_SCHEME_RE.search(value[token_start:index])
    return match is not None and match.group(0).casefold() != "file://"


def _root_occurrence_is_path(value: str, index: int) -> bool:
    if index == 0 or value[index - 1] in "=,:; \t\r\n'\"()[]{}":
        return True
    prefix = value[max(0, index - 16) : index]
    return any(prefix.endswith(flag) for flag in _JOINED_PATH_PREFIXES)


def _filesystem_root_pattern(root: Path) -> re.Pattern[str]:
    rendered = root.as_posix().rstrip("/")
    if not rendered:
        raise ValueError("filesystem root cannot be a location identity authority")
    components = rendered.split("/")
    prefix = ""
    if len(components) >= 2 and components[0].endswith(":") and not components[1]:
        components.pop(1)
    if rendered.startswith("//"):
        while components and not components[0]:
            components.pop(0)
        prefix = r"(?<!:)//+"
    body = "/+".join(re.escape(component) for component in components)
    return re.compile(
        prefix + body + r"""(?:(?P<separator>/+)|(?=$|[=;,\s'"\)\]\}]))""",
        re.IGNORECASE if root.drive else 0,
    )


def _residual_producer_paths(value: Any, *, location: str = "$") -> list[str]:
    findings: list[str] = []

    def inspect(text: str, item_location: str) -> None:
        normalized = text.replace("\\", "/")
        if _FILE_URL_RE.search(normalized):
            findings.append(f"{item_location}: residual file URL in {text!r}")
            return
        if _HOME_PATH_RE.search(normalized):
            findings.append(
                f"{item_location}: residual home-relative producer path in {text!r}"
            )
            return
        for pattern, kind in (
            (_WINDOWS_ABSOLUTE_RE, "drive"),
            (_UNC_ABSOLUTE_RE, "UNC"),
            (_POSIX_ABSOLUTE_RE, "POSIX"),
        ):
            for match in pattern.finditer(normalized):
                if (
                    kind == "POSIX"
                    and match.start() == 0
                    and normalized.upper().startswith(
                        tuple(prefix.upper() for prefix in _MSVC_PATH_FLAG_PREFIXES)
                    )
                ):
                    continue
                if not _inside_url_token(normalized, match.start()):
                    findings.append(
                        f"{item_location}: residual {kind} producer path in {text!r}"
                    )
                    return
        for match in _JOINED_PATH_FLAG_RE.finditer(normalized):
            if not _inside_url_token(normalized, match.start("path")):
                findings.append(
                    f"{item_location}: residual joined-flag producer path in {text!r}"
                )
                return

    def walk(item: Any, item_location: str) -> None:
        if isinstance(item, str):
            inspect(item, item_location)
        elif isinstance(item, list):
            for index, child in enumerate(item):
                walk(child, f"{item_location}[{index}]")
        elif isinstance(item, Mapping):
            for raw_key, child in item.items():
                key = str(raw_key)
                inspect(key, f"{item_location}.<key>")
                walk(child, f"{item_location}.{key}")

    walk(value, location)
    return findings


def _require_location_neutral(value: Any, *, authority: str) -> None:
    findings = _residual_producer_paths(value)
    if findings:
        preview = "; ".join(findings[:8])
        suffix = "" if len(findings) <= 8 else f"; +{len(findings) - 8} more"
        raise ValueError(
            f"{authority} retains producer filesystem paths: {preview}{suffix}"
        )


def _ordered_location_roots(
    roots: Sequence[tuple[Path | None, str]],
) -> tuple[tuple[Path, str], ...]:
    deduped: list[tuple[Path, str]] = []
    seen: set[Path] = set()
    for path, replacement in roots:
        if path is None:
            continue
        resolved = path.resolve()
        if resolved not in seen:
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
            key=lambda indexed: (-ancestor_count(indexed[1][0]), indexed[0]),
        )
    )


def _source_extension_deterministic_path_args(
    *,
    compiler_command: Sequence[str],
    roots: Sequence[tuple[Path | None, str]],
) -> list[str]:
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
    value: str, location_roots: Sequence[tuple[Path | None, str]]
) -> str:
    return _canonicalize_location_string_ordered(
        value, _ordered_location_roots(location_roots)
    )


def _canonicalize_location_string_ordered(
    value: str, ordered_roots: Sequence[tuple[Path, str]]
) -> str:
    canonical = value.replace("\\", "/")
    for root, token in ordered_roots:
        pattern = _filesystem_root_pattern(root)

        def replace(match: re.Match[str]) -> str:
            if _inside_url_token(
                canonical, match.start()
            ) or not _root_occurrence_is_path(canonical, match.start()):
                return match.group(0)
            return token + ("/" if match.group("separator") else "")

        canonical = pattern.sub(replace, canonical)
    return canonical


def _canonicalize_locations(
    value: Any,
    location_roots: Sequence[tuple[Path | None, str]],
    source_paths: Mapping[Path, str] | None = None,
) -> Any:
    ordered_roots = _ordered_location_roots(location_roots)
    resolved_sources: dict[str, str] = {}
    for path, replacement in (source_paths or {}).items():
        for candidate in (path.expanduser(), path.resolve()):
            key = os.path.normcase(os.path.normpath(os.fspath(candidate)))
            prior = resolved_sources.setdefault(key, replacement)
            if prior != replacement:
                raise ValueError(
                    "source-path canonicalization has conflicting identities"
                )

    def canonicalize(item: Any) -> Any:
        if isinstance(item, str):
            expanded = os.path.expanduser(item)
            candidate = (
                os.path.normcase(os.path.normpath(expanded))
                if source_paths is not None and os.path.isabs(expanded)
                else None
            )
            if candidate is not None and candidate in resolved_sources:
                return resolved_sources[candidate]
            return _canonicalize_location_string_ordered(item, ordered_roots)
        if isinstance(item, list):
            return [canonicalize(child) for child in item]
        if isinstance(item, dict):
            canonical: dict[str, Any] = {}
            for raw_key, child in item.items():
                key = _canonicalize_location_string_ordered(str(raw_key), ordered_roots)
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
    value: Any, location_roots: Sequence[tuple[Path | None, str]]
) -> Any:
    canonical = _canonicalize_locations(value, location_roots)
    dependency_ids: dict[str, str] = {}

    def replace(item: Any, *, in_dependencies: bool = False) -> Any:
        if isinstance(item, str):
            if not in_dependencies or not _MESON_TRANSIENT_DEPENDENCY_ID_RE.fullmatch(
                item
            ):
                return item
            return dependency_ids.setdefault(
                item, f"@meson-dependency/{len(dependency_ids):04d}"
            )
        if isinstance(item, list):
            return [replace(child, in_dependencies=in_dependencies) for child in item]
        if isinstance(item, dict):
            return {
                key: replace(item[key], in_dependencies=key == "dependencies")
                for key in sorted(item)
            }
        return item

    return replace(canonical)


def _canonical_json_sha256(
    path: Path,
    *,
    location_roots: Sequence[tuple[Path | None, str]],
    normalize_meson_dependency_ids: bool,
) -> str:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError(
            f"cannot canonicalize JSON identity input {path}: {exc}"
        ) from exc
    canonical = (
        _canonicalize_meson_metadata(payload, location_roots)
        if normalize_meson_dependency_ids
        else _canonicalize_locations(payload, location_roots)
    )
    return hashlib.sha256(
        json.dumps(canonical, sort_keys=True, indent=2).encode("utf-8") + b"\n"
    ).hexdigest()


def _canonical_extension_manifest_for_wheel(
    manifest: Mapping[str, Any],
    *,
    location_roots: Sequence[tuple[Path | None, str]],
    meson_plan_path: Path | None = None,
    compile_commands_path: Path | None = None,
) -> dict[str, Any]:
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
        identity = dict(source_plan)
        identity.pop("digest", None)
        source_plan["digest"] = hashlib.sha256(
            json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        if isinstance(canonical.get("build"), dict):
            canonical["build"]["source_plan_digest"] = source_plan["digest"]
    closure = canonical.get("object_closure")
    if isinstance(closure, dict) and "closure_sha256" in closure:
        identity = dict(closure)
        identity.pop("closure_sha256", None)
        closure["closure_sha256"] = hashlib.sha256(
            json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        if isinstance(canonical.get("build"), dict):
            canonical["build"]["object_closure_sha256"] = closure["closure_sha256"]
    return canonical
