"""Verified NumPy/SciPy/CPython compatibility-matrix authority."""

from __future__ import annotations

import os
import re
import tomllib
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from collections.abc import Mapping
from typing import Any

from molt.source_root import compiler_source_root
from molt.cli.source_extension_set_registry import (
    SourceExtensionRegistry,
    SourceExtensionSet,
    SourceExtensionVariant,
    load_source_extension_registry,
    source_extension_custody_root,
    source_extension_set_root,
)
from molt.cli.source_extension_set_validation import (
    ValidatedSourceExtensionSetSeal,
    validate_source_extension_set_seal,
)
from molt.cli.source_extension_target import resolve_source_extension_target_plan
from molt.target_python import (
    _parse_target_python_version,
    require_supported_target_python,
)

CONFIG_ENV = "MOLT_SCIENTIFIC_STACK_CONFIG"
SCIENTIFIC_EXTENSION_ABI_TIER = "cpython-abi"
# The pyproject dependency group whose locked environment runs the Pact
# witness lanes' CPython reference numerics; its pins are bound to the
# selected stack versions by tests/tools/test_scientific_stack_versions.py.
PACT_WITNESS_DEPENDENCY_GROUP = "pact-witness"

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


@dataclass(frozen=True, slots=True)
class ValidatedScientificExtensionSeals:
    """Exact registered NumPy and SciPy seal receipts for one target variant."""

    variant: SourceExtensionVariant
    numpy: ValidatedSourceExtensionSetSeal
    scipy: ValidatedSourceExtensionSetSeal

    @property
    def payload_roots(self) -> tuple[Path, Path]:
        return (self.numpy.payload_root, self.scipy.payload_root)

    def receipt(self, package: str) -> ValidatedSourceExtensionSetSeal:
        if package == "numpy":
            return self.numpy
        if package == "scipy":
            return self.scipy
        raise ValueError(f"no scientific extension seal for package {package!r}")


def _config_path(config_path: Path | None) -> Path:
    if config_path is not None:
        return config_path
    override = os.environ.get(CONFIG_ENV)
    return (
        Path(override)
        if override
        else compiler_source_root() / "config" / "scientific_stack_versions.toml"
    )


def _require_exact_keys(
    value: Mapping[Any, Any], *, expected: set[str], field: str, path: Path
) -> None:
    actual = set(value)
    if actual != expected:
        raise ValueError(
            f"{path}: {field} keys are invalid: "
            f"missing={sorted(expected - actual)!r}, unknown={sorted(actual - expected)!r}"
        )


def _table(value: object, *, field: str, path: Path) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{path}: {field} must be a table")
    table: dict[str, Any] = {}
    for key, item in value.items():
        if not isinstance(key, str):
            raise ValueError(f"{path}: {field} keys must be strings")
        table[key] = item
    return table


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
    raw: Mapping[Any, Any],
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
        target_python = require_supported_target_python(
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
    witness_sets: dict[str, SourceExtensionSet] = {}
    for package in ("numpy", "scipy"):
        matches = tuple(
            item
            for item in extension_sets
            if item.package == package and item.name == "pact-witness"
        )
        if len(matches) != 1:
            raise ValueError(
                f"{path}: {field}.extension_sets must reference exactly one "
                f"{package}/pact-witness set"
            )
        witness_sets[package] = matches[0]
    numpy_variants = {
        expectation.variant.coordinate for expectation in witness_sets["numpy"].variants
    }
    scipy_variants = {
        expectation.variant.coordinate for expectation in witness_sets["scipy"].variants
    }
    if numpy_variants != scipy_variants:
        raise ValueError(
            f"{path}: {field} scientific extension variant coordinates differ: "
            f"numpy-only={sorted(numpy_variants - scipy_variants)!r}, "
            f"scipy-only={sorted(scipy_variants - numpy_variants)!r}"
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
    selection = _table(payload.get("selection"), field="[selection]", path=path)
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
        table = _table(raw, field=f"verified[{index}]", path=path)
        entry = _scientific_entry(table, index=index, path=path, registry=registry)
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


def scientific_extension_variant(
    target: str, *, stack: ScientificStackVersion | None = None
) -> SourceExtensionVariant:
    selected = resolve_scientific_stack() if stack is None else stack
    target_plan = resolve_source_extension_target_plan(target)
    variant = SourceExtensionVariant(
        target_python=_parse_target_python_version(selected.cpython),
        abi_tier=SCIENTIFIC_EXTENSION_ABI_TIER,
        target_triple=target_plan.target_triple,
    )
    for package in ("numpy", "scipy"):
        extension_set = selected.extension_set(package, "pact-witness")
        if not any(
            expectation.variant == variant for expectation in extension_set.variants
        ):
            raise ValueError(
                "no canonical identity is registered for scientific extension "
                f"variant {package}/{extension_set.package_version}/pact-witness/"
                f"{variant.cpython}/{variant.abi_tier}/{variant.target_triple}"
            )
    return variant


def scientific_extension_seal_root(
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


def validate_scientific_extension_seals(
    target: str, *, stack: ScientificStackVersion | None = None
) -> ValidatedScientificExtensionSeals:
    """Validate both current registered seal identities for the exact target."""

    selected = resolve_scientific_stack() if stack is None else stack
    variant = scientific_extension_variant(target, stack=selected)
    receipts: dict[str, ValidatedSourceExtensionSetSeal] = {}
    for package in ("numpy", "scipy"):
        extension_set = selected.extension_set(package, "pact-witness")
        durable_root = scientific_extension_seal_root(
            package, variant=variant, stack=selected
        )
        if not durable_root.exists():
            raise ValueError(
                f"canonical {package} scientific extension seal does not exist for "
                f"{variant.coordinate!r}: {durable_root}"
            )
        try:
            receipts[package] = validate_source_extension_set_seal(
                durable_root,
                extension_set,
                variant=variant,
                registry=selected.source_extension_registry,
            )
        except ValueError as exc:
            raise ValueError(
                f"canonical {package} scientific extension seal is invalid for "
                f"{variant.coordinate!r}: {exc}"
            ) from exc
    return ValidatedScientificExtensionSeals(
        variant=variant, numpy=receipts["numpy"], scipy=receipts["scipy"]
    )
