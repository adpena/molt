"""Generic package-set, variant, source, and seal-custody authority."""

from __future__ import annotations

import keyword
import os
import re
import subprocess
import tomllib
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from molt.target_python import TargetPythonVersion, _parse_target_python_version
from molt.dx import checkout_custody

ROOT = Path(__file__).resolve().parents[3]
DEFAULT_CONFIG_PATH = ROOT / "config" / "source_extension_package_sets.toml"
CONFIG_ENV = "MOLT_SOURCE_EXTENSION_SET_REGISTRY_CONFIG"
SOURCE_EXTENSION_EXEC_CAPABILITY = "module.extension.exec"

_PUBLIC_VERSION_RE = re.compile(r"^[0-9]+(?:\.[0-9]+)+$")
_CUSTODY_COMPONENT_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
_GIT_COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
_WINDOWS_RESERVED_FILENAME = re.compile(
    r"(?i)\A(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\..*)?\Z"
)


@dataclass(frozen=True, slots=True)
class SourceExtensionSpec:
    module: str
    target: str
    python_exports: tuple[str, ...]
    capabilities: tuple[str, ...]
    provided_capsules: tuple[str, ...]
    exclude_linked_static_libraries: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class SourceExtensionVariant:
    target_python: TargetPythonVersion
    abi_tier: str
    target_triple: str

    def __post_init__(self) -> None:
        if not isinstance(self.target_python, TargetPythonVersion):
            raise TypeError("source-extension variant requires TargetPythonVersion")
        for field, value in (
            ("abi_tier", self.abi_tier),
            ("target_triple", self.target_triple),
        ):
            if value != value.strip().lower() or not _CUSTODY_COMPONENT_RE.fullmatch(
                value
            ):
                raise ValueError(
                    f"source-extension variant {field} must be a canonical "
                    f"lowercase custody component, got {value!r}"
                )

    @property
    def cpython(self) -> str:
        return self.target_python.short

    @property
    def coordinate(self) -> tuple[str, str, str]:
        return (self.cpython, self.abi_tier, self.target_triple)


@dataclass(frozen=True, slots=True)
class SourceExtensionVariantExpectation:
    variant: SourceExtensionVariant
    expected_identity_sha256: str


@dataclass(frozen=True, slots=True)
class SourceExtensionSource:
    kind: str
    commit: str


@dataclass(frozen=True, slots=True)
class SourceExtensionSet:
    package: str
    package_version: str
    source: SourceExtensionSource
    name: str
    seal_name: str
    variants: tuple[SourceExtensionVariantExpectation, ...]
    build_dependency_group: str
    meson_setup_args: tuple[str, ...]
    use_pkg_config: bool
    required_installed_files: tuple[str, ...]
    extensions: tuple[SourceExtensionSpec, ...]
    required_config_tools: tuple[str, ...] = ()

    @property
    def coordinate(self) -> tuple[str, str, str]:
        return (self.package, self.package_version, self.name)


@dataclass(frozen=True, slots=True)
class SourceExtensionPackage:
    name: str
    version: str
    source: SourceExtensionSource
    sets: tuple[SourceExtensionSet, ...]


@dataclass(frozen=True, slots=True)
class SourceExtensionRegistry:
    schema_version: int
    packages: tuple[SourceExtensionPackage, ...]
    path: Path

    def package(self, name: str, version: str) -> SourceExtensionPackage:
        for package in self.packages:
            if (package.name, package.version) == (name, version):
                return package
        raise ValueError(
            f"no source-extension package {name} {version} is registered in {self.path}"
        )

    def extension_set(
        self, package: str, version: str, name: str
    ) -> SourceExtensionSet:
        registered_package = self.package(package, version)
        for extension_set in registered_package.sets:
            if extension_set.name == name:
                return extension_set
        raise ValueError(
            f"no source-extension set {package} {version}/{name} is registered "
            f"in {self.path}"
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


def _component(value: Any, *, field: str, path: Path) -> str:
    text = _string(value, field=field, path=path)
    if _CUSTODY_COMPONENT_RE.fullmatch(text) is None:
        raise ValueError(
            f"{path}: {field} must be a lowercase portable custody component"
        )
    return text


def _version(value: Any, *, field: str, path: Path) -> str:
    text = _string(value, field=field, path=path)
    if _PUBLIC_VERSION_RE.fullmatch(text) is None:
        raise ValueError(f"{path}: {field} must be a dotted numeric version")
    return text


def _string_tuple(
    value: Any,
    *,
    field: str,
    path: Path,
    allow_empty: bool,
    canonical_set: bool,
) -> tuple[str, ...]:
    if not isinstance(value, list) or (not value and not allow_empty):
        requirement = "a string array" if allow_empty else "a non-empty string array"
        raise ValueError(f"{path}: {field} must be {requirement}")
    parsed = tuple(_string(item, field=field, path=path) for item in value)
    if canonical_set and tuple(sorted(set(parsed))) != parsed:
        raise ValueError(f"{path}: {field} must be sorted and duplicate-free")
    return parsed


def _relative_path(value: Any, *, field: str, path: Path) -> str:
    text = _string(value, field=field, path=path)
    relative = PurePosixPath(text)
    if (
        relative.is_absolute()
        or text in {".", ".."}
        or ".." in relative.parts
        or "\\" in text
        or "\x00" in text
        or str(relative) != text
    ):
        raise ValueError(
            f"{path}: {field} must be a canonical root-relative POSIX path"
        )
    for component in relative.parts:
        if (
            component.endswith((".", " "))
            or _WINDOWS_RESERVED_FILENAME.fullmatch(component) is not None
        ):
            raise ValueError(f"{path}: {field} is not portable across filesystems")
    return text


def validate_source_extension_module_target(
    module: object, target: object
) -> tuple[str, str]:
    if (
        not isinstance(module, str)
        or not module
        or not all(
            part.isidentifier() and not keyword.iskeyword(part)
            for part in module.split(".")
        )
    ):
        raise ValueError(f"source-extension module is not import syntax: {module!r}")
    if (
        not isinstance(target, str)
        or not target
        or target in {".", ".."}
        or any(ord(character) < 32 for character in target)
        or any(character in target for character in '<>:"/\\|?*')
        or target.startswith(("~", "$", "%"))
        or target.endswith((".", " "))
        or _WINDOWS_RESERVED_FILENAME.fullmatch(target) is not None
    ):
        raise ValueError(f"source-extension target is not a safe filename: {target!r}")
    return module, target


def _variant_expectations(
    value: Any, *, field: str, path: Path
) -> tuple[SourceExtensionVariantExpectation, ...]:
    if not isinstance(value, list) or not value:
        raise ValueError(f"{path}: {field} must be a non-empty table array")
    result: list[SourceExtensionVariantExpectation] = []
    seen: set[tuple[str, str, str]] = set()
    for index, raw in enumerate(value):
        item_field = f"{field}[{index}]"
        if not isinstance(raw, dict):
            raise ValueError(f"{path}: {item_field} must be a table")
        _require_exact_keys(
            raw,
            expected={
                "cpython",
                "abi_tier",
                "target_triple",
                "expected_identity_sha256",
            },
            field=item_field,
            path=path,
        )
        try:
            target_python = _parse_target_python_version(
                _version(raw.get("cpython"), field=f"{item_field}.cpython", path=path)
            )
        except ValueError as exc:
            raise ValueError(f"{path}: {item_field}.cpython is invalid: {exc}") from exc
        variant = SourceExtensionVariant(
            target_python=target_python,
            abi_tier=_component(
                raw.get("abi_tier"), field=f"{item_field}.abi_tier", path=path
            ),
            target_triple=_component(
                raw.get("target_triple"),
                field=f"{item_field}.target_triple",
                path=path,
            ),
        )
        if variant.coordinate in seen:
            raise ValueError(
                f"{path}: duplicate source-extension variant {variant.coordinate!r}"
            )
        seen.add(variant.coordinate)
        digest = _string(
            raw.get("expected_identity_sha256"),
            field=f"{item_field}.expected_identity_sha256",
            path=path,
        )
        if _SHA256_RE.fullmatch(digest) is None:
            raise ValueError(
                f"{path}: {item_field}.expected_identity_sha256 must be lowercase SHA-256"
            )
        result.append(SourceExtensionVariantExpectation(variant, digest))
    return tuple(result)


def _extension_specs(
    value: Any, *, field: str, path: Path
) -> tuple[SourceExtensionSpec, ...]:
    if not isinstance(value, list) or not value:
        raise ValueError(f"{path}: {field} must be a non-empty table array")
    result: list[SourceExtensionSpec] = []
    seen_modules: set[str] = set()
    seen_artifacts: set[str] = set()
    for index, raw in enumerate(value):
        item_field = f"{field}[{index}]"
        if not isinstance(raw, dict):
            raise ValueError(f"{path}: {item_field} must be a table")
        _require_exact_keys(
            raw,
            expected={
                "module",
                "target",
                "python_exports",
                "capabilities",
                "provided_capsules",
                "exclude_linked_static_libraries",
            },
            field=item_field,
            path=path,
        )
        module, target = validate_source_extension_module_target(
            raw.get("module"), raw.get("target")
        )
        module_key = module.casefold()
        artifact_key = "/".join((*module.split(".")[:-1], target)).casefold()
        if module_key in seen_modules:
            raise ValueError(f"{path}: duplicate extension module {module!r}")
        if artifact_key in seen_artifacts:
            raise ValueError(
                f"{path}: case-folded extension artifact collision for {module!r}"
            )
        seen_modules.add(module_key)
        seen_artifacts.add(artifact_key)
        capabilities = _string_tuple(
            raw.get("capabilities"),
            field=f"{item_field}.capabilities",
            path=path,
            allow_empty=False,
            canonical_set=True,
        )
        if SOURCE_EXTENSION_EXEC_CAPABILITY not in capabilities:
            raise ValueError(
                f"{path}: {item_field}.capabilities must include "
                f"{SOURCE_EXTENSION_EXEC_CAPABILITY!r}"
            )
        result.append(
            SourceExtensionSpec(
                module=module,
                target=target,
                python_exports=_string_tuple(
                    raw.get("python_exports"),
                    field=f"{item_field}.python_exports",
                    path=path,
                    allow_empty=False,
                    canonical_set=True,
                ),
                capabilities=capabilities,
                provided_capsules=_string_tuple(
                    raw.get("provided_capsules"),
                    field=f"{item_field}.provided_capsules",
                    path=path,
                    allow_empty=True,
                    canonical_set=True,
                ),
                exclude_linked_static_libraries=_string_tuple(
                    raw.get("exclude_linked_static_libraries"),
                    field=f"{item_field}.exclude_linked_static_libraries",
                    path=path,
                    allow_empty=True,
                    canonical_set=True,
                ),
            )
        )
    return tuple(result)


def load_source_extension_registry(
    config_path: Path | None = None,
) -> SourceExtensionRegistry:
    path = _config_path(config_path).resolve()
    try:
        payload = tomllib.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise ValueError(
            f"failed to read source-extension registry {path}: {exc}"
        ) from exc
    except tomllib.TOMLDecodeError as exc:
        raise ValueError(f"invalid source-extension registry {path}: {exc}") from exc
    _require_exact_keys(
        payload,
        expected={"schema_version", "packages"},
        field="root",
        path=path,
    )
    if payload.get("schema_version") != 1:
        raise ValueError(f"{path}: schema_version must be 1")
    raw_packages = payload.get("packages")
    if not isinstance(raw_packages, list) or not raw_packages:
        raise ValueError(f"{path}: at least one [[packages]] entry is required")
    packages: list[SourceExtensionPackage] = []
    seen_packages: set[tuple[str, str]] = set()
    for package_index, raw_package in enumerate(raw_packages):
        package_field = f"packages[{package_index}]"
        if not isinstance(raw_package, dict):
            raise ValueError(f"{path}: {package_field} must be a table")
        _require_exact_keys(
            raw_package,
            expected={"name", "version", "source", "sets"},
            field=package_field,
            path=path,
        )
        name = _component(
            raw_package.get("name"), field=f"{package_field}.name", path=path
        )
        version = _version(
            raw_package.get("version"), field=f"{package_field}.version", path=path
        )
        package_key = (name.casefold(), version)
        if package_key in seen_packages:
            raise ValueError(f"{path}: duplicate package coordinate {name} {version}")
        seen_packages.add(package_key)
        raw_source = raw_package.get("source")
        if not isinstance(raw_source, dict):
            raise ValueError(f"{path}: {package_field}.source must be a table")
        _require_exact_keys(
            raw_source,
            expected={"kind", "commit"},
            field=f"{package_field}.source",
            path=path,
        )
        source_kind = _component(
            raw_source.get("kind"),
            field=f"{package_field}.source.kind",
            path=path,
        )
        source_commit = _string(
            raw_source.get("commit"),
            field=f"{package_field}.source.commit",
            path=path,
        )
        if source_kind != "git" or _GIT_COMMIT_RE.fullmatch(source_commit) is None:
            raise ValueError(
                f"{path}: {package_field}.source must be a git source with a "
                "lowercase 40-hex commit"
            )
        source = SourceExtensionSource(source_kind, source_commit)
        raw_sets = raw_package.get("sets")
        if not isinstance(raw_sets, list) or not raw_sets:
            raise ValueError(f"{path}: {package_field}.sets must be a non-empty array")
        sets: list[SourceExtensionSet] = []
        seen_set_names: set[str] = set()
        seen_seal_names: set[str] = set()
        for set_index, raw_set in enumerate(raw_sets):
            set_field = f"{package_field}.sets[{set_index}]"
            if not isinstance(raw_set, dict):
                raise ValueError(f"{path}: {set_field} must be a table")
            _require_exact_keys(
                raw_set,
                expected={
                    "name",
                    "seal_name",
                    "variants",
                    "build_dependency_group",
                    "meson_setup_args",
                    "use_pkg_config",
                    "required_config_tools",
                    "required_installed_files",
                    "extensions",
                },
                field=set_field,
                path=path,
            )
            set_name = _component(
                raw_set.get("name"), field=f"{set_field}.name", path=path
            )
            seal_name = _component(
                raw_set.get("seal_name"), field=f"{set_field}.seal_name", path=path
            )
            if set_name.casefold() in seen_set_names:
                raise ValueError(f"{path}: duplicate extension set {name}/{set_name}")
            if seal_name.casefold() in seen_seal_names:
                raise ValueError(f"{path}: duplicate extension seal path {seal_name}")
            seen_set_names.add(set_name.casefold())
            seen_seal_names.add(seal_name.casefold())
            use_pkg_config = raw_set.get("use_pkg_config")
            if not isinstance(use_pkg_config, bool):
                raise ValueError(f"{path}: {set_field}.use_pkg_config must be boolean")
            required_installed_files = _string_tuple(
                raw_set.get("required_installed_files"),
                field=f"{set_field}.required_installed_files",
                path=path,
                allow_empty=False,
                canonical_set=True,
            )
            for item_index, item in enumerate(required_installed_files):
                _relative_path(
                    item,
                    field=f"{set_field}.required_installed_files[{item_index}]",
                    path=path,
                )
            required_config_tools = _string_tuple(
                raw_set.get("required_config_tools"),
                field=f"{set_field}.required_config_tools",
                path=path,
                allow_empty=True,
                canonical_set=True,
            )
            for tool_index, tool_name in enumerate(required_config_tools):
                _component(
                    tool_name,
                    field=f"{set_field}.required_config_tools[{tool_index}]",
                    path=path,
                )
            if use_pkg_config != ("pkg-config" in required_config_tools):
                raise ValueError(
                    f"{path}: {set_field}.use_pkg_config must exactly match "
                    "required_config_tools membership for pkg-config"
                )
            sets.append(
                SourceExtensionSet(
                    package=name,
                    package_version=version,
                    source=source,
                    name=set_name,
                    seal_name=seal_name,
                    variants=_variant_expectations(
                        raw_set.get("variants"),
                        field=f"{set_field}.variants",
                        path=path,
                    ),
                    build_dependency_group=_component(
                        raw_set.get("build_dependency_group"),
                        field=f"{set_field}.build_dependency_group",
                        path=path,
                    ),
                    meson_setup_args=_string_tuple(
                        raw_set.get("meson_setup_args"),
                        field=f"{set_field}.meson_setup_args",
                        path=path,
                        allow_empty=False,
                        canonical_set=False,
                    ),
                    use_pkg_config=use_pkg_config,
                    required_installed_files=required_installed_files,
                    extensions=_extension_specs(
                        raw_set.get("extensions"),
                        field=f"{set_field}.extensions",
                        path=path,
                    ),
                    required_config_tools=required_config_tools,
                )
            )
        packages.append(SourceExtensionPackage(name, version, source, tuple(sets)))
    return SourceExtensionRegistry(1, tuple(packages), path)


def source_extension_set(
    package: str,
    package_version: str,
    name: str,
    *,
    registry: SourceExtensionRegistry | None = None,
) -> SourceExtensionSet:
    selected = load_source_extension_registry() if registry is None else registry
    return selected.extension_set(package, package_version, name)


def require_registered_source_extension_set(
    extension_set: SourceExtensionSet,
    *,
    registry: SourceExtensionRegistry | None = None,
) -> SourceExtensionSet:
    selected = load_source_extension_registry() if registry is None else registry
    registered = selected.extension_set(*extension_set.coordinate)
    if extension_set != registered:
        raise ValueError(
            f"source-extension set {extension_set.coordinate!r} differs from the "
            f"registry authority {selected.path}"
        )
    return registered


def source_extension_set_expected_identity(
    extension_set: SourceExtensionSet,
    *,
    variant: SourceExtensionVariant,
    registry: SourceExtensionRegistry | None = None,
) -> str:
    registered = require_registered_source_extension_set(
        extension_set, registry=registry
    )
    for expectation in registered.variants:
        if expectation.variant == variant:
            return expectation.expected_identity_sha256
    raise ValueError(
        "no canonical identity is registered for source-extension variant "
        f"{registered.package}/{registered.package_version}/{registered.name}/"
        f"{variant.cpython}/{variant.abi_tier}/{variant.target_triple}"
    )


def source_extension_custody_root() -> Path:
    return checkout_custody(ROOT, os.environ).custody_root


def source_extension_set_root(
    extension_set: SourceExtensionSet,
    *,
    variant: SourceExtensionVariant,
    registry: SourceExtensionRegistry | None = None,
) -> Path:
    source_extension_set_expected_identity(
        extension_set,
        variant=variant,
        registry=registry,
    )
    return (
        source_extension_custody_root()
        / "package-seals"
        / extension_set.package
        / extension_set.package_version
        / "variants"
        / f"cpython-{variant.cpython}"
        / variant.abi_tier
        / variant.target_triple
        / extension_set.seal_name
    )


def verify_source_extension_checkout(
    extension_set: SourceExtensionSet,
    root: Path,
    *,
    registry: SourceExtensionRegistry | None = None,
) -> None:
    registered = require_registered_source_extension_set(
        extension_set, registry=registry
    )
    result = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    actual = result.stdout.strip()
    if result.returncode != 0 or actual != registered.source.commit:
        detail = actual or result.stderr.strip() or f"returncode={result.returncode}"
        raise ValueError(
            f"{registered.package} source checkout {root} does not match registered "
            f"commit {registered.source.commit}: got {detail}"
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
            f"{registered.package} source checkout {root} is not a clean immutable "
            f"input: {detail}{suffix}"
        )


def verify_source_extension_abi_headers(
    variant: SourceExtensionVariant, *, repo_root: Path = ROOT
) -> None:
    if variant.abi_tier != "cpython-abi":
        return
    python_h = repo_root / "runtime" / "molt-cpython-abi" / "include" / "Python.h"
    text = python_h.read_text(encoding="utf-8")
    major_match = re.search(r"^#define PY_MAJOR_VERSION ([0-9]+)$", text, re.MULTILINE)
    minor_match = re.search(r"^#define PY_MINOR_VERSION ([0-9]+)$", text, re.MULTILINE)
    actual = (
        f"{major_match.group(1)}.{minor_match.group(1)}"
        if major_match and minor_match
        else "<unresolved>"
    )
    if actual != variant.cpython:
        raise ValueError(
            f"source-extension variant requires CPython {variant.cpython}, but "
            f"{python_h} declares {actual}"
        )
