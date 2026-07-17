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
from molt.dx import checkout_custody

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CONFIG_PATH = ROOT / "config" / "scientific_stack_versions.toml"
CONFIG_ENV = "MOLT_SCIENTIFIC_STACK_CONFIG"
_PUBLIC_VERSION_RE = re.compile(r"^[0-9]+(?:\.[0-9]+)+$")
SCIENTIFIC_EXTENSION_EXEC_CAPABILITY = "module.extension.exec"


@dataclass(frozen=True)
class ScientificExtensionSpec:
    module: str
    target: str
    python_exports: tuple[str, ...]
    capabilities: tuple[str, ...]
    provided_capsules: tuple[str, ...] = ()
    exclude_linked_static_libraries: tuple[str, ...] = ()


@dataclass(frozen=True)
class ScientificExtensionSet:
    package: str
    name: str
    seal_name: str
    expected_identity_sha256: str
    build_dependency_group: str
    meson_setup_args: tuple[str, ...]
    use_pkg_config: bool
    required_installed_files: tuple[str, ...]
    extensions: tuple[ScientificExtensionSpec, ...]


@dataclass(frozen=True)
class ScientificStackVersion:
    numpy: str
    scipy: str
    cpython: str
    numpy_repo_ref: str
    scipy_repo_ref: str
    extension_sets: tuple[ScientificExtensionSet, ...]

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


def _capability_tuple(value: Any, *, field: str, path: Path) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise ValueError(f"{path}: {field} must be a string array")
    capabilities = tuple(_string(item, field=field, path=path) for item in value)
    if tuple(sorted(set(capabilities))) != capabilities:
        raise ValueError(
            f"{path}: {field} must be sorted and contain no duplicate capabilities"
        )
    return capabilities


def _boolean(value: Any, *, field: str, path: Path) -> bool:
    if not isinstance(value, bool):
        raise ValueError(f"{path}: {field} must be a boolean")
    return value


def _sha256(value: Any, *, field: str, path: Path) -> str:
    text = _string(value, field=field, path=path)
    if not re.fullmatch(r"[0-9a-f]{64}", text):
        raise ValueError(f"{path}: {field} must be a lowercase SHA-256 digest")
    return text


def _extension_sets(
    value: Any, *, field: str, path: Path
) -> tuple[ScientificExtensionSet, ...]:
    if not isinstance(value, list) or not value:
        raise ValueError(f"{path}: {field} must be a non-empty table array")
    sets: list[ScientificExtensionSet] = []
    seen: set[tuple[str, str]] = set()
    for set_index, raw_set in enumerate(value):
        set_field = f"{field}[{set_index}]"
        if not isinstance(raw_set, dict):
            raise ValueError(f"{path}: {set_field} must be a table")
        package = _string(
            raw_set.get("package"), field=f"{set_field}.package", path=path
        )
        name = _string(raw_set.get("name"), field=f"{set_field}.name", path=path)
        key = (package, name)
        if key in seen:
            raise ValueError(f"{path}: duplicate extension set {package}/{name}")
        seen.add(key)
        raw_extensions = raw_set.get("extensions")
        if not isinstance(raw_extensions, list) or not raw_extensions:
            raise ValueError(
                f"{path}: {set_field}.extensions must be a non-empty table array"
            )
        extensions: list[ScientificExtensionSpec] = []
        seen_modules: set[str] = set()
        seen_targets: set[str] = set()
        for extension_index, raw_extension in enumerate(raw_extensions):
            extension_field = f"{set_field}.extensions[{extension_index}]"
            if not isinstance(raw_extension, dict):
                raise ValueError(f"{path}: {extension_field} must be a table")
            module = _string(
                raw_extension.get("module"),
                field=f"{extension_field}.module",
                path=path,
            )
            target = _string(
                raw_extension.get("target"),
                field=f"{extension_field}.target",
                path=path,
            )
            python_exports = _string_tuple(
                raw_extension.get("python_exports"),
                field=f"{extension_field}.python_exports",
                path=path,
            )
            capabilities = _capability_tuple(
                raw_extension.get("capabilities"),
                field=f"{extension_field}.capabilities",
                path=path,
            )
            if SCIENTIFIC_EXTENSION_EXEC_CAPABILITY not in capabilities:
                raise ValueError(
                    f"{path}: {extension_field}.capabilities must include "
                    f"{SCIENTIFIC_EXTENSION_EXEC_CAPABILITY!r}"
                )
            provided_capsules = _capability_tuple(
                raw_extension.get("provided_capsules", []),
                field=f"{extension_field}.provided_capsules",
                path=path,
            )
            exclude_linked_static_libraries = _capability_tuple(
                raw_extension.get("exclude_linked_static_libraries", []),
                field=f"{extension_field}.exclude_linked_static_libraries",
                path=path,
            )
            if module in seen_modules:
                raise ValueError(f"{path}: duplicate extension module {module}")
            if target in seen_targets:
                raise ValueError(f"{path}: duplicate extension target {target}")
            seen_modules.add(module)
            seen_targets.add(target)
            extensions.append(
                ScientificExtensionSpec(
                    module=module,
                    target=target,
                    python_exports=python_exports,
                    capabilities=capabilities,
                    provided_capsules=provided_capsules,
                    exclude_linked_static_libraries=(
                        exclude_linked_static_libraries
                    ),
                )
            )
        sets.append(
            ScientificExtensionSet(
                package=package,
                name=name,
                seal_name=_string(
                    raw_set.get("seal_name"),
                    field=f"{set_field}.seal_name",
                    path=path,
                ),
                expected_identity_sha256=_sha256(
                    raw_set.get("expected_identity_sha256"),
                    field=f"{set_field}.expected_identity_sha256",
                    path=path,
                ),
                build_dependency_group=_string(
                    raw_set.get("build_dependency_group"),
                    field=f"{set_field}.build_dependency_group",
                    path=path,
                ),
                meson_setup_args=_string_tuple(
                    raw_set.get("meson_setup_args"),
                    field=f"{set_field}.meson_setup_args",
                    path=path,
                ),
                use_pkg_config=_boolean(
                    raw_set.get("use_pkg_config"),
                    field=f"{set_field}.use_pkg_config",
                    path=path,
                ),
                required_installed_files=_capability_tuple(
                    raw_set.get("required_installed_files"),
                    field=f"{set_field}.required_installed_files",
                    path=path,
                ),
                extensions=tuple(extensions),
            )
        )
    return tuple(sets)


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
    if payload.get("schema_version") != 4:
        raise ValueError(f"{path}: schema_version must be 4")
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
        removed_fields = {
            "numpy_seal_root_candidates",
            "scipy_primary_seal_root_candidates",
            "scipy_additional_seal_roots",
        }.intersection(raw)
        if removed_fields:
            fields = ", ".join(sorted(removed_fields))
            raise ValueError(
                f"{path}: verified[{index}] uses removed schema-v1 fields: {fields}; "
                "declare a typed extension_sets entry instead"
            )
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
            extension_sets=_extension_sets(
                raw.get("extension_sets"),
                field=f"verified[{index}].extension_sets",
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


def scientific_custody_root() -> Path:
    """Package-seal custody, independent of source-checkout location."""
    return checkout_custody(ROOT, os.environ).custody_root


def numpy_witness_seal_root(
    *,
    stack: ScientificStackVersion | None = None,
) -> Path:
    selected = resolve_scientific_stack() if stack is None else stack
    return scientific_extension_set_root(
        scientific_extension_set("numpy", "pact-witness", stack=selected),
        stack=selected,
    )


def scientific_extension_set(
    package: str,
    name: str,
    stack: ScientificStackVersion | None = None,
) -> ScientificExtensionSet:
    selected = resolve_scientific_stack() if stack is None else stack
    for extension_set in selected.extension_sets:
        if (extension_set.package, extension_set.name) == (package, name):
            return extension_set
    raise ValueError(
        f"no scientific extension set {package}/{name} in {selected.tuple_label}"
    )


def scientific_extension_set_root(
    extension_set: ScientificExtensionSet,
    stack: ScientificStackVersion | None = None,
) -> Path:
    selected = resolve_scientific_stack() if stack is None else stack
    configured = scientific_extension_set(
        extension_set.package, extension_set.name, selected
    )
    if extension_set != configured:
        raise ValueError(
            f"scientific extension set {extension_set.package}/{extension_set.name} "
            "does not match the selected verified-stack authority"
        )
    version = {"numpy": selected.numpy, "scipy": selected.scipy}.get(
        extension_set.package
    )
    if version is None:
        raise ValueError(
            f"unsupported scientific extension package {extension_set.package!r}"
        )
    root = scientific_custody_root()
    return (
        root
        / "package-seals"
        / extension_set.package
        / version
        / extension_set.seal_name
    )


def scipy_witness_seal_root(
    *,
    stack: ScientificStackVersion | None = None,
) -> Path:
    selected = resolve_scientific_stack() if stack is None else stack
    return scientific_extension_set_root(
        scientific_extension_set("scipy", "pact-witness", stack=selected),
        stack=selected,
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
    status = subprocess.run(
        [
            "git",
            "-C",
            str(root),
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    dirty = tuple(line for line in status.stdout.splitlines() if line.strip())
    if status.returncode != 0 or dirty:
        detail = (
            "; ".join(dirty[:8])
            or status.stderr.strip()
            or f"returncode={status.returncode}"
        )
        suffix = "" if len(dirty) <= 8 else f"; +{len(dirty) - 8} more"
        raise ValueError(
            f"{package} source checkout {root} is not a clean immutable input: "
            f"{detail}{suffix}"
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
