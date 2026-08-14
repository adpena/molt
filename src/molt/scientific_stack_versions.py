"""Verified NumPy/SciPy/CPython compatibility-matrix authority."""

from __future__ import annotations

import ast
import os
import re
import tomllib
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from molt.cli.source_extension_set_registry import (
    SourceExtensionRegistry,
    SourceExtensionSet,
    SourceExtensionVariant,
    load_source_extension_registry,
    source_extension_custody_root,
    source_extension_set_root,
)
from molt.target_python import (
    _parse_target_python_version,
    require_known_cpython_coverage_version,
)

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CONFIG_PATH = ROOT / "config" / "scientific_stack_versions.toml"
CONFIG_ENV = "MOLT_SCIENTIFIC_STACK_CONFIG"
SCIENTIFIC_WITNESS_TARGET_TRIPLE = "wasm32-wasip1"
SCIENTIFIC_WITNESS_ABI_TIER = "cpython-abi"

_PUBLIC_VERSION_RE = re.compile(r"^[0-9]+(?:\.[0-9]+)+$")


@dataclass(frozen=True, slots=True)
class ScientificStackVersion:
    numpy: str
    scipy: str
    cpython: str
    numpy_repo_ref: str
    scipy_repo_ref: str
    extension_sets: tuple[SourceExtensionSet, ...]
    source_extension_registry: SourceExtensionRegistry

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

    def extension_set(self, package: str, name: str) -> SourceExtensionSet:
        for extension_set in self.extension_sets:
            if (extension_set.package, extension_set.name) == (package, name):
                return extension_set
        raise ValueError(
            f"no scientific extension set {package}/{name} in {self.tuple_label}"
        )


def _config_path(config_path: Path | None) -> Path:
    if config_path is not None:
        return config_path
    override = os.environ.get(CONFIG_ENV)
    return Path(override) if override else DEFAULT_CONFIG_PATH


def _require_exact_keys(
    value: dict[str, Any], *, expected: set[str], field: str, path: Path
) -> None:
    actual = set(value)
    if actual != expected:
        raise ValueError(
            f"{path}: {field} keys are invalid: "
            f"missing={sorted(expected - actual)!r}, unknown={sorted(actual - expected)!r}"
        )


def _string(value: Any, *, field: str, path: Path) -> str:
    if not isinstance(value, str) or not value.strip() or value != value.strip():
        raise ValueError(f"{path}: {field} must be a canonical non-empty string")
    return value


def _version(value: Any, *, field: str, path: Path) -> str:
    text = _string(value, field=field, path=path)
    if _PUBLIC_VERSION_RE.fullmatch(text) is None:
        raise ValueError(f"{path}: {field} must be a dotted numeric version")
    return text


def _string_set(value: Any, *, field: str, path: Path) -> tuple[str, ...]:
    if not isinstance(value, list) or not value:
        raise ValueError(f"{path}: {field} must be a non-empty string array")
    values = tuple(_string(item, field=field, path=path) for item in value)
    if tuple(sorted(set(values))) != values:
        raise ValueError(f"{path}: {field} must be sorted and duplicate-free")
    return values


def _registry_path(value: Any, *, path: Path) -> Path:
    raw = _string(value, field="source_extension_registry", path=path)
    relative = PurePosixPath(raw)
    if (
        relative.is_absolute()
        or raw in {".", ".."}
        or ".." in relative.parts
        or "\\" in raw
        or str(relative) != raw
    ):
        raise ValueError(
            f"{path}: source_extension_registry must be a canonical sibling-relative path"
        )
    return (path.parent / Path(*relative.parts)).resolve()


def _extension_set_ref(value: str, *, field: str, path: Path) -> tuple[str, str]:
    parts = value.split("/")
    if len(parts) != 2 or not all(parts):
        raise ValueError(f"{path}: {field} must be package/set")
    return parts[0], parts[1]


def _scientific_entry(
    raw: dict[str, Any],
    *,
    index: int,
    path: Path,
    registry: SourceExtensionRegistry,
) -> ScientificStackVersion:
    field = f"verified[{index}]"
    _require_exact_keys(
        raw,
        expected={"numpy", "scipy", "cpython", "extension_sets"},
        field=field,
        path=path,
    )
    numpy = _version(raw.get("numpy"), field=f"{field}.numpy", path=path)
    scipy = _version(raw.get("scipy"), field=f"{field}.scipy", path=path)
    cpython = _version(raw.get("cpython"), field=f"{field}.cpython", path=path)
    try:
        target_python = require_known_cpython_coverage_version(
            _parse_target_python_version(cpython)
        )
    except ValueError as exc:
        raise ValueError(f"{path}: {field}.cpython is invalid: {exc}") from exc
    package_versions = {"numpy": numpy, "scipy": scipy}
    extension_sets: list[SourceExtensionSet] = []
    for ref_index, raw_ref in enumerate(
        _string_set(
            raw.get("extension_sets"), field=f"{field}.extension_sets", path=path
        )
    ):
        package_name, set_name = _extension_set_ref(
            raw_ref,
            field=f"{field}.extension_sets[{ref_index}]",
            path=path,
        )
        package_version = package_versions.get(package_name)
        if package_version is None:
            raise ValueError(
                f"{path}: {field}.extension_sets references non-scientific "
                f"package {package_name!r}"
            )
        extension_set = registry.extension_set(package_name, package_version, set_name)
        if not any(
            expectation.variant.target_python == target_python
            for expectation in extension_set.variants
        ):
            raise ValueError(
                f"{path}: {raw_ref} has no registered CPython {cpython} variant"
            )
        extension_sets.append(extension_set)
    referenced_packages = {item.package for item in extension_sets}
    if referenced_packages != set(package_versions):
        raise ValueError(
            f"{path}: {field}.extension_sets must cover numpy and scipy exactly"
        )
    numpy_package = registry.package("numpy", numpy)
    scipy_package = registry.package("scipy", scipy)
    return ScientificStackVersion(
        numpy=numpy,
        scipy=scipy,
        cpython=cpython,
        numpy_repo_ref=numpy_package.source.commit,
        scipy_repo_ref=scipy_package.source.commit,
        extension_sets=tuple(extension_sets),
        source_extension_registry=registry,
    )


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
    _require_exact_keys(
        payload,
        expected={
            "schema_version",
            "source_extension_registry",
            "selection",
            "verified",
        },
        field="root",
        path=path,
    )
    if payload.get("schema_version") != 6:
        raise ValueError(f"{path}: schema_version must be 6")
    registry = load_source_extension_registry(
        _registry_path(payload.get("source_extension_registry"), path=path)
    )
    selection = payload.get("selection")
    if not isinstance(selection, dict):
        raise ValueError(f"{path}: [selection] table is required")
    _require_exact_keys(
        selection,
        expected={"numpy", "scipy", "cpython"},
        field="selection",
        path=path,
    )
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
        entry = _scientific_entry(raw, index=index, path=path, registry=registry)
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


def scientific_custody_root() -> Path:
    return source_extension_custody_root()


def scientific_witness_variant(
    *, stack: ScientificStackVersion | None = None
) -> SourceExtensionVariant:
    selected = resolve_scientific_stack() if stack is None else stack
    return SourceExtensionVariant(
        target_python=_parse_target_python_version(selected.cpython),
        abi_tier=SCIENTIFIC_WITNESS_ABI_TIER,
        target_triple=SCIENTIFIC_WITNESS_TARGET_TRIPLE,
    )


def scientific_witness_seal_root(
    package: str,
    *,
    variant: SourceExtensionVariant,
    stack: ScientificStackVersion | None = None,
) -> Path:
    selected = resolve_scientific_stack() if stack is None else stack
    return source_extension_set_root(
        selected.extension_set(package, "pact-witness"),
        variant=variant,
        registry=selected.source_extension_registry,
    )


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
    root: Path, *, stack: ScientificStackVersion | None = None
) -> str:
    selected = resolve_scientific_stack() if stack is None else stack
    effective = read_numpy_seal_version(root)
    if effective != selected.numpy:
        raise ValueError(
            f"NumPy seal attestation failed: configured={selected.numpy} "
            f"effective={effective} root={root}"
        )
    return effective
