"""Verified NumPy/SciPy/CPython version authority for package custody."""

from __future__ import annotations

import ast
import os
import re
import subprocess
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from molt.cli.target_python import (
    TargetPythonVersion,
    require_known_cpython_coverage_version,
)

ROOT = Path(__file__).resolve().parents[2]
NUMPY_WITNESS_SEAL_NAME = "pact_numpy_multiarray_sealed_for_witness"
DEFAULT_CONFIG_PATH = ROOT / "config" / "scientific_stack_versions.toml"
CONFIG_ENV = "MOLT_SCIENTIFIC_STACK_CONFIG"
_PUBLIC_VERSION_RE = re.compile(r"^[0-9]+(?:\.[0-9]+)+$")


@dataclass(frozen=True)
class ScientificStackVersion:
    numpy: str
    scipy: str
    cpython: str
    numpy_repo_ref: str
    scipy_repo_ref: str
    numpy_seal_root_candidates: tuple[str, ...]
    scipy_primary_seal_root_candidates: tuple[str, ...]
    scipy_additional_seal_roots: tuple[str, ...]

    @property
    def seal_roots(self) -> tuple[str, ...]:
        return (
            *self.numpy_seal_root_candidates,
            *self.scipy_primary_seal_root_candidates,
            *self.scipy_additional_seal_roots,
        )

    @property
    def numpy_requirement(self) -> str:
        return f"numpy=={self.numpy}"

    @property
    def scipy_requirement(self) -> str:
        return f"scipy=={self.scipy}"

    @property
    def tuple_label(self) -> str:
        return f"numpy {self.numpy}/scipy {self.scipy}/cpython {self.cpython}"

    def substitutions(self) -> dict[str, str]:
        return {
            "scientific_numpy_version": self.numpy,
            "scientific_scipy_version": self.scipy,
            "scientific_cpython_version": self.cpython,
            "scientific_numpy_requirement": self.numpy_requirement,
            "scientific_scipy_requirement": self.scipy_requirement,
            "scientific_numpy_repo_ref": self.numpy_repo_ref,
            "scientific_scipy_repo_ref": self.scipy_repo_ref,
        }


def _config_path(config_path: Path | None) -> Path:
    if config_path is not None:
        return config_path
    override = os.environ.get(CONFIG_ENV)
    return Path(override) if override else DEFAULT_CONFIG_PATH


def _version(value: Any, *, field: str, path: Path) -> str:
    if not isinstance(value, str) or not _PUBLIC_VERSION_RE.fullmatch(value):
        raise ValueError(f"{path}: {field} must be a dotted numeric version")
    return value


def _string(value: Any, *, field: str, path: Path) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{path}: {field} must be a non-empty string")
    return value.strip()


def _string_tuple(value: Any, *, field: str, path: Path) -> tuple[str, ...]:
    if not isinstance(value, list) or not value:
        raise ValueError(f"{path}: {field} must be a non-empty string array")
    return tuple(_string(item, field=field, path=path) for item in value)


def load_verified_support_matrix(
    config_path: Path | None = None,
) -> tuple[tuple[str, str, str], list[ScientificStackVersion], Path]:
    path = _config_path(config_path).resolve()
    try:
        payload = tomllib.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise ValueError(
            f"failed to read scientific-stack config {path}: {exc}"
        ) from exc
    except tomllib.TOMLDecodeError as exc:
        raise ValueError(f"invalid scientific-stack config {path}: {exc}") from exc
    if payload.get("schema_version") != 1:
        raise ValueError(f"{path}: schema_version must be 1")
    selection = payload.get("selection")
    if not isinstance(selection, dict):
        raise ValueError(f"{path}: [selection] table is required")
    selected = (
        _version(selection.get("numpy"), field="selection.numpy", path=path),
        _version(selection.get("scipy"), field="selection.scipy", path=path),
        _version(selection.get("cpython"), field="selection.cpython", path=path),
    )
    raw_entries = payload.get("verified")
    if not isinstance(raw_entries, list) or not raw_entries:
        raise ValueError(f"{path}: at least one [[verified]] entry is required")
    entries: list[ScientificStackVersion] = []
    seen: set[tuple[str, str, str]] = set()
    for index, raw in enumerate(raw_entries):
        if not isinstance(raw, dict):
            raise ValueError(f"{path}: verified[{index}] must be a table")
        entry = ScientificStackVersion(
            numpy=_version(
                raw.get("numpy"), field=f"verified[{index}].numpy", path=path
            ),
            scipy=_version(
                raw.get("scipy"), field=f"verified[{index}].scipy", path=path
            ),
            cpython=_version(
                raw.get("cpython"), field=f"verified[{index}].cpython", path=path
            ),
            numpy_repo_ref=_string(
                raw.get("numpy_repo_ref"),
                field=f"verified[{index}].numpy_repo_ref",
                path=path,
            ),
            scipy_repo_ref=_string(
                raw.get("scipy_repo_ref"),
                field=f"verified[{index}].scipy_repo_ref",
                path=path,
            ),
            numpy_seal_root_candidates=_string_tuple(
                raw.get("numpy_seal_root_candidates"),
                field=f"verified[{index}].numpy_seal_root_candidates",
                path=path,
            ),
            scipy_primary_seal_root_candidates=_string_tuple(
                raw.get("scipy_primary_seal_root_candidates"),
                field=f"verified[{index}].scipy_primary_seal_root_candidates",
                path=path,
            ),
            scipy_additional_seal_roots=_string_tuple(
                raw.get("scipy_additional_seal_roots"),
                field=f"verified[{index}].scipy_additional_seal_roots",
                path=path,
            ),
        )
        key = (entry.numpy, entry.scipy, entry.cpython)
        if key in seen:
            raise ValueError(f"{path}: duplicate verified tuple {entry.tuple_label}")
        seen.add(key)
        entries.append(entry)
    return selected, entries, path


def resolve_scientific_stack(
    config_path: Path | None = None,
) -> ScientificStackVersion:
    selected, entries, path = load_verified_support_matrix(config_path)
    for entry in entries:
        if selected == (entry.numpy, entry.scipy, entry.cpython):
            major, minor = (int(part) for part in entry.cpython.split(".", 1))
            require_known_cpython_coverage_version(TargetPythonVersion(major, minor, 0))
            return entry
    verified = ", ".join(entry.tuple_label for entry in entries)
    numpy, scipy, cpython = selected
    raise ValueError(
        f"numpy {numpy}/scipy {scipy}/cpython {cpython} is not in Molt's "
        f"verified-support matrix; verified: {verified}. Update {path} only "
        "after producing and verifying matching package seals."
    )


def apply_scientific_stack_substitutions(value: str) -> str:
    if "{scientific_" not in value:
        return value
    stack = resolve_scientific_stack()
    try:
        return value.format_map(stack.substitutions())
    except KeyError as exc:
        raise ValueError(
            f"unknown scientific-stack placeholder {exc.args[0]!r}"
        ) from exc


def scientific_artifact_root(env: dict[str, str] | None = None) -> Path:
    env_view = os.environ if env is None else env
    configured = env_view.get("MOLT_EXT_ROOT", "").strip()
    if configured:
        return Path(configured).expanduser().resolve()
    roots = env_view.get("MOLT_EXTERNAL_ARTIFACT_ROOTS", "").strip()
    if roots:
        first = next(
            (item.strip() for item in roots.split(os.pathsep) if item.strip()), ""
        )
        if first:
            return Path(first).expanduser().resolve()
    raise ValueError(
        "scientific package seals require MOLT_EXT_ROOT or "
        "MOLT_EXTERNAL_ARTIFACT_ROOTS so effective artifacts have durable shared custody"
    )


def numpy_witness_seal_root(
    *,
    stack: ScientificStackVersion | None = None,
    artifact_root: Path | None = None,
) -> Path:
    selected = resolve_scientific_stack() if stack is None else stack
    root = scientific_artifact_root() if artifact_root is None else artifact_root.resolve()
    return root / "package-seals" / "numpy" / selected.numpy / NUMPY_WITNESS_SEAL_NAME


def read_numpy_seal_version(root: Path) -> str:
    version_path = root / "numpy" / "version.py"
    try:
        tree = ast.parse(
            version_path.read_text(encoding="utf-8"), filename=str(version_path)
        )
    except (OSError, SyntaxError) as exc:
        raise ValueError(
            f"cannot read effective NumPy seal version from {version_path}: {exc}"
        ) from exc
    for node in tree.body:
        if not isinstance(node, ast.Assign):
            continue
        if not any(
            isinstance(target, ast.Name) and target.id == "version"
            for target in node.targets
        ):
            continue
        if isinstance(node.value, ast.Constant) and isinstance(node.value.value, str):
            return node.value.value
        break
    raise ValueError(
        f"effective NumPy seal has no literal version assignment: {version_path}"
    )


def attest_numpy_witness_seal(
    root: Path,
    *,
    stack: ScientificStackVersion | None = None,
) -> str:
    selected = resolve_scientific_stack() if stack is None else stack
    effective = read_numpy_seal_version(root)
    if effective != selected.numpy:
        raise ValueError(
            f"NumPy seal attestation failed: configured={selected.numpy} "
            f"effective={effective} root={root}"
        )
    return effective


def verify_source_checkout(
    package: str, root: Path, *, stack: ScientificStackVersion | None = None
) -> None:
    selected = resolve_scientific_stack() if stack is None else stack
    expected = {"numpy": selected.numpy_repo_ref, "scipy": selected.scipy_repo_ref}.get(
        package
    )
    if expected is None:
        raise ValueError(f"unsupported scientific package {package!r}")
    result = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    actual = result.stdout.strip()
    if result.returncode != 0 or actual != expected:
        detail = actual or result.stderr.strip() or f"returncode={result.returncode}"
        raise ValueError(
            f"{package} source checkout {root} does not match verified "
            f"{selected.tuple_label}: expected {expected}, got {detail}"
        )


def verify_cpython_abi_headers(
    *, stack: ScientificStackVersion | None = None, repo_root: Path = ROOT
) -> None:
    selected = resolve_scientific_stack() if stack is None else stack
    python_h = repo_root / "runtime" / "molt-cpython-abi" / "include" / "Python.h"
    text = python_h.read_text(encoding="utf-8")
    major_match = re.search(r"^#define PY_MAJOR_VERSION ([0-9]+)$", text, re.MULTILINE)
    minor_match = re.search(r"^#define PY_MINOR_VERSION ([0-9]+)$", text, re.MULTILINE)
    actual = (
        f"{major_match.group(1)}.{minor_match.group(1)}"
        if major_match and minor_match
        else "<unresolved>"
    )
    if actual != selected.cpython:
        raise ValueError(
            f"verified scientific stack requires CPython {selected.cpython}, but "
            f"{python_h} declares {actual}"
        )
