from __future__ import annotations

import ast
import platform as platform_module
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

from packaging.specifiers import InvalidSpecifier, SpecifierSet
from packaging.version import InvalidVersion, Version


@dataclass(frozen=True, slots=True)
class TargetPythonVersion:
    major: int
    minor: int
    micro: int
    release: str = "final"
    serial: int = 0

    @property
    def feature_version(self) -> tuple[int, int]:
        return (self.major, self.minor)

    @property
    def short(self) -> str:
        return f"{self.major}.{self.minor}"

    @property
    def tag(self) -> str:
        return f"py{self.major}{self.minor}"


_SUPPORTED_TARGET_PYTHON_VERSIONS: tuple[TargetPythonVersion, ...] = (
    TargetPythonVersion(3, 12, 0),
    TargetPythonVersion(3, 13, 0),
    TargetPythonVersion(3, 14, 0),
)
_SUPPORTED_TARGET_PYTHON_BY_SHORT = {
    version.short: version for version in _SUPPORTED_TARGET_PYTHON_VERSIONS
}
_DEFAULT_TARGET_PYTHON_VERSION = _SUPPORTED_TARGET_PYTHON_BY_SHORT["3.12"]
# Single authority for the set of supported target-Python short versions
# ("3.12", "3.13", ...). This is the `TargetPythonVersion` authority; the
# stdlib-coverage tooling (tools/stdlib_module_union.py baseline and
# tools/gen_stdlib_module_union.py generator) must NOT re-declare this list —
# they derive from or are gated against this tuple by
# tools/check_table_drift.py so a version bump here cannot silently drift stdlib
# coverage.
SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS: tuple[str, ...] = tuple(
    version.short for version in _SUPPORTED_TARGET_PYTHON_VERSIONS
)


def _target_platform_name(value: str | None = None) -> str:
    raw = (platform_module.system() if value is None else value).strip().lower()
    return {"win32": "windows", "darwin": "macos"}.get(raw, raw)


def required_verified_subset_python_tuples() -> tuple[tuple[str, str], ...]:
    from molt.verified_subset import verified_subset_coordinates

    return tuple(
        sorted(
            {
                (coordinate.python, coordinate.platform)
                for coordinate in verified_subset_coordinates()
            }
        )
    )


def require_supported_target_python(
    target: TargetPythonVersion,
) -> TargetPythonVersion:
    supported = set(SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS)
    if target.short not in supported:
        rendered = ", ".join(sorted(supported))
        raise ValueError(
            f"CPython {target.short} is outside the supported target set: {rendered}"
        )
    return target


def require_verified_subset_target(
    target: TargetPythonVersion, *, platform: str | None = None
) -> TargetPythonVersion:
    selected = (target.short, _target_platform_name(platform))
    required = required_verified_subset_python_tuples()
    if selected not in required:
        rendered = ", ".join(
            f"CPython {version} on {name}" for version, name in required
        )
        raise ValueError(
            f"Molt target tuple is outside the required verified-subset matrix: "
            f"CPython {selected[0]} on {selected[1]}; required tuples: {rendered}. "
            "A tuple is proven only by a passing source-bound receipt."
        )
    return target


def _parse_target_python_version(value: str | None) -> TargetPythonVersion:
    if value is None or not value.strip():
        return _DEFAULT_TARGET_PYTHON_VERSION
    raw = value.strip().lower()
    if raw.startswith("py") and len(raw) == 5 and raw[2:].isdigit():
        raw = f"{raw[2]}.{raw[3:]}"
    try:
        parsed = Version(raw)
    except InvalidVersion as exc:
        raise ValueError(f"invalid Python target version {value!r}") from exc
    key = f"{parsed.major}.{parsed.minor}"
    target = _SUPPORTED_TARGET_PYTHON_BY_SHORT.get(key)
    if target is None:
        supported = ", ".join(
            version.short for version in _SUPPORTED_TARGET_PYTHON_VERSIONS
        )
        raise ValueError(
            f"unsupported Python target version {value!r}; supported versions: {supported}"
        )
    return target


def _project_requires_python(project_root: Path) -> str | None:
    pyproject = project_root / "pyproject.toml"
    if not pyproject.exists():
        return None
    try:
        data = tomllib.loads(pyproject.read_text(encoding="utf-8"))
    except OSError as exc:
        raise ValueError(
            f"failed to read pyproject.toml at {pyproject}: {exc}"
        ) from exc
    except tomllib.TOMLDecodeError as exc:
        raise ValueError(f"invalid pyproject.toml at {pyproject}: {exc}") from exc
    project_cfg = data.get("project")
    if project_cfg is None:
        return None
    if not isinstance(project_cfg, dict):
        raise ValueError(f"project table in {pyproject} must be a TOML table")
    if "requires-python" not in project_cfg:
        return None
    raw = project_cfg["requires-python"]
    if not isinstance(raw, str) or not raw.strip():
        raise ValueError(
            f"project.requires-python in {pyproject} must be a non-empty string"
        )
    return raw


def _target_python_from_requires_python(
    requires_python: str | None,
) -> TargetPythonVersion:
    if not requires_python:
        return _DEFAULT_TARGET_PYTHON_VERSION
    try:
        specifier = SpecifierSet(requires_python)
    except InvalidSpecifier as exc:
        raise ValueError(
            f"invalid project.requires-python specifier {requires_python!r}"
        ) from exc
    for target in _SUPPORTED_TARGET_PYTHON_VERSIONS:
        if Version(target.short) in specifier:
            return target
    supported = ", ".join(
        version.short for version in _SUPPORTED_TARGET_PYTHON_VERSIONS
    )
    raise ValueError(
        f"project.requires-python {requires_python!r} does not admit any "
        f"supported Molt target ({supported})"
    )


def _resolve_target_python_version(
    *,
    explicit: str | None,
    build_config: Mapping[str, Any] | None,
    project_root: Path,
) -> TargetPythonVersion:
    if explicit is not None and explicit.strip():
        return _parse_target_python_version(explicit)
    if build_config is not None:
        for key in (
            "python_version",
            "python-version",
            "target_python",
            "target-python",
        ):
            if key not in build_config:
                continue
            raw_config = build_config[key]
            if not isinstance(raw_config, str) or not raw_config.strip():
                raise ValueError(f"[tool.molt.build] {key} must be a non-empty string")
            return _parse_target_python_version(raw_config)
    return _target_python_from_requires_python(_project_requires_python(project_root))


def _parse_source_for_target(
    source: str,
    *,
    filename: str = "<unknown>",
    target_python: TargetPythonVersion,
) -> ast.AST:
    frontend_version = (sys.version_info.major, sys.version_info.minor)
    if frontend_version < target_python.feature_version:
        raise SyntaxError(
            f"Molt target Python {target_python.short} requires a Python "
            f"{target_python.short}+ frontend; run the build with "
            f"`uv run --python {target_python.short} -m molt.cli ...`"
        )
    return ast.parse(
        source,
        filename=filename,
        feature_version=target_python.feature_version,
    )
