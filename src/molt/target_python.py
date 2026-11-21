from __future__ import annotations

import ast
import sys
from dataclasses import dataclass

from molt import target_python_policy
from molt.host_python import require_frontend_python
from molt._python_release_generated import CPYTHON_RELEASES


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


_SUPPORTED_TARGET_PYTHON_VERSIONS: tuple[TargetPythonVersion, ...] = tuple(
    TargetPythonVersion(*CPYTHON_RELEASES[short])
    for short in target_python_policy.SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
)
_SUPPORTED_TARGET_PYTHON_BY_SHORT = {
    version.short: version for version in _SUPPORTED_TARGET_PYTHON_VERSIONS
}
_DEFAULT_TARGET_PYTHON_VERSION = _SUPPORTED_TARGET_PYTHON_BY_SHORT[
    target_python_policy.DEFAULT_TARGET_PYTHON_SHORT_VERSION
]


def require_supported_target_python(
    target: TargetPythonVersion,
) -> TargetPythonVersion:
    supported = set(target_python_policy.SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS)
    if target.short not in supported:
        rendered = ", ".join(sorted(supported))
        raise ValueError(
            f"CPython {target.short} is outside the supported target set: {rendered}"
        )
    return target


def _parse_target_python_version(value: str | None) -> TargetPythonVersion:
    if value is None or not value.strip():
        return _DEFAULT_TARGET_PYTHON_VERSION
    raw = value.strip().lower()
    if raw.startswith("py"):
        matching = [
            target for target in _SUPPORTED_TARGET_PYTHON_VERSIONS if target.tag == raw
        ]
        if len(matching) == 1:
            raw = matching[0].short
        elif matching:
            raise ValueError(f"ambiguous Python target tag {value!r}; use major.minor")
    from packaging.version import InvalidVersion, Version

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


def resolve_target_python_for_oracle(
    oracle_version: tuple[int, int] | None,
    explicit: str | TargetPythonVersion | None = None,
) -> TargetPythonVersion:
    """Require the compiled language minor to match the measured CPython oracle."""
    if oracle_version is None:
        raise ValueError("cannot derive Molt target Python from the CPython oracle")
    derived = _parse_target_python_version(f"{oracle_version[0]}.{oracle_version[1]}")
    if explicit is None:
        return derived
    target = (
        require_supported_target_python(explicit)
        if isinstance(explicit, TargetPythonVersion)
        else _parse_target_python_version(explicit)
    )
    if isinstance(explicit, str) and explicit != target.short:
        raise ValueError(
            "Molt target Python must use canonical <major>.<minor> spelling"
        )
    if target.feature_version != oracle_version:
        raise ValueError(
            "Molt target Python does not match the CPython oracle: "
            f"target={target.short}, oracle={derived.short}"
        )
    return target


def _parse_source_for_target(
    source: str | bytes,
    *,
    filename: str = "<unknown>",
    target_python: TargetPythonVersion,
) -> ast.Module:
    require_frontend_python()
    frontend_version = (sys.version_info.major, sys.version_info.minor)
    # CPython grammar grows by addition between admitted minors: every source
    # an older parser accepts has the same AST under the target's parser
    # (measured 2026-11-17 over 9,320 files of CPython 3.12.16 and 3.15.2 Lib
    # and Molt's tree: zero AST differences, zero sources accepted by 3.12 and
    # refused by 3.15). An older frontend therefore compiles what it parses;
    # only syntax it cannot parse needs a frontend as new as the target.
    try:
        tree = ast.parse(
            source,
            filename=filename,
            feature_version=target_python.feature_version,
        )
    except SyntaxError as exc:
        if frontend_version >= target_python.feature_version:
            raise
        host = f"{frontend_version[0]}.{frontend_version[1]}"
        raise SyntaxError(
            f"{exc.msg}; the Python {host} frontend cannot parse Python "
            f"{target_python.short} syntax. Molt target Python "
            f"{target_python.short} source needs a Python "
            f"{target_python.short}+ frontend; for a source checkout, run "
            f"`uv run --python {target_python.short} molt build <source> "
            f"--python-version {target_python.short}`. For a binary bundle, "
            "set PYTHON to a CPython "
            f"{target_python.short}+ executable, run "
            "`molt setup --install-cli-dependencies`, then retry `molt build`",
            (
                exc.filename,
                exc.lineno,
                exc.offset,
                exc.text,
                exc.end_lineno,
                exc.end_offset,
            ),
        ) from exc
    from molt.compiler_analysis.python_syntax import (
        PYTHON_SYNTAX_DELTAS,
        PYTHON_SYNTAX_NODE_TYPES,
        require_python_syntax_target,
    )

    if any(frontend_version >= delta.minimum_version for delta in PYTHON_SYNTAX_DELTAS):
        for node in ast.walk(tree):
            if isinstance(node, PYTHON_SYNTAX_NODE_TYPES):
                require_python_syntax_target(
                    node, target_python.feature_version, filename=filename
                )
    return tree
