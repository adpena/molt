from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from functools import lru_cache
import hashlib
import json
import os
import platform
from pathlib import Path, PureWindowsPath
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import tomllib
import uuid
from dataclasses import asdict, dataclass
from typing import Any, Literal

from molt.file_hashing import _content_change_time_ns
from molt.llvm_linker_roles import (
    executable_entrypoint_name,
    executable_selects_linker_role,
    host_llvm_linker_role,
    is_llvm_linker_role,
    lexical_executable_path,
)
from molt.wasi_sysroot import normalize_wasi_sysroot, wasi_sysroot_llvm_version


class LlvmToolchainConfigError(RuntimeError):
    """Raised when Cargo's LLVM feature pin cannot be resolved uniquely."""


@dataclass(frozen=True)
class LlvmBackendPin:
    major: int
    minor: int
    inkwell_feature: str
    inkwell_manifest: str
    llvm_sys_version: str | None

    @property
    def env_var(self) -> str:
        return llvm_sys_prefix_env_var(self.major, self.minor)

    @property
    def default_release(self) -> str:
        return default_llvm_release(self.major, self.minor)


@dataclass(frozen=True)
class LlvmHostArchitecture:
    id: str
    aliases: tuple[str, ...]
    llvm_target: str
    rust_cfg: str
    inkwell_feature: str
    cranelift_feature: str | None = None
    cranelift_architecture: str | None = None
    windows_component: str | None = None
    windows_target_arch: str | None = None
    windows_host_arch: str | None = None


@dataclass(frozen=True)
class LlvmArchitectureContract:
    schema_version: int
    required_projects: tuple[str, ...]
    required_targets: tuple[str, ...]
    architectures: tuple[LlvmHostArchitecture, ...]
    digest: str


@dataclass(frozen=True)
class LlvmManagedPaths:
    root: Path
    prefix: Path
    archive: Path
    source_root: Path
    build_dir: Path


@dataclass(frozen=True)
class LlvmToolchainDiscovery:
    """The executable and prefix identity reported by one LLVM installation.

    Package managers are allowed to install a versioned ``llvm-config`` outside
    the SDK prefix (Debian/Ubuntu use ``/usr/bin/llvm-config-<major>`` for an
    SDK rooted at ``/usr/lib/llvm-<major>``).  Keeping both paths is therefore
    part of the toolchain identity; deriving one from the other is incorrect.
    """

    prefix: Path
    llvm_config: Path
    version: str
    source: str


@dataclass(frozen=True)
class LlvmRelease:
    version: str
    url: str
    size: int
    source_sha256: str
    provenance_url: str
    minimum_cmake: str
    record_sha256: str


@dataclass(frozen=True)
class LlvmDebianInstaller:
    url: str
    sha256: str


@dataclass(frozen=True)
class WasiSysrootRelease:
    version: str
    llvm_version: str
    url: str
    size: int
    sha256: str
    provenance_url: str
    archive_root: str
    record_sha256: str


@dataclass(frozen=True)
class LlvmReleaseManifest:
    schema_version: int
    default_release: str
    canonical_build_type: str
    debian_installer: LlvmDebianInstaller
    wasi_sysroot: WasiSysrootRelease
    releases: tuple[LlvmRelease, ...]
    digest: str


@dataclass(frozen=True)
class LlvmPrefixVerification:
    prefix: Path
    llvm_config: Path
    version: str
    targets: tuple[str, ...]
    assets: tuple[str, ...]
    tool_versions: tuple["LlvmToolVersionFact", ...]
    library_facts: tuple["LlvmLibraryFact", ...]
    content_facts: tuple["LlvmContentFact", ...]
    content_digest: str | None
    link_closure: tuple[str, ...]
    link_probe: tuple[str, ...]
    release: LlvmRelease | None


@dataclass(frozen=True)
class WasmCiToolchainVerification:
    llvm_prefix: Path
    wasm_ld: Path
    wasm_ld_fact: "LlvmToolVersionFact"
    sysroot: Path
    sysroot_version: str
    sysroot_llvm_version: str
    sysroot_assets: tuple[str, ...]


@dataclass(frozen=True)
class LlvmToolVersionFact:
    role: str
    path: str
    version: str
    size: int
    sha256: str


@dataclass(frozen=True)
class LlvmLibraryFact:
    path: str
    size: int
    mtime_ns: int


@dataclass(frozen=True)
class LlvmContentFact:
    path: str
    size: int
    mtime_ns: int
    change_ns: int | None
    sha256: str


LLVM_ATTESTATION_SCHEMA = "molt.llvm-toolchain.v5"
LLVM_ATTESTATION_FILENAME = ".molt-llvm-toolchain.json"


def reject_poison_toolchain_path(raw: str | Path, *, authority: str) -> None:
    """Reject the retired D: custody root before host path normalization.

    ``Path.resolve`` on a non-Windows review host turns ``D:\\...`` into a
    relative POSIX path, so inspect the lexical Windows drive first.  This is a
    repository-authority invariant, not a host-platform convenience rule.
    """

    rendered = str(raw).strip()
    drive = PureWindowsPath(rendered).drive.upper()
    normalized = rendered.replace("/", "\\").upper()
    if drive == "D:" or re.match(r"^(?:\\\\[?.]\\|\\\?\?\\)D:\\", normalized):
        raise LlvmToolchainConfigError(
            f"{authority} cannot use retired D: canonical custody: {raw}"
        )


def llvm_architecture_contract_path(root: Path) -> Path:
    return root.resolve() / "config" / "llvm_toolchain_arches.toml"


def llvm_release_manifest_path(root: Path | None = None) -> Path:
    resolved_root = (
        root.resolve() if root is not None else Path(__file__).resolve().parents[2]
    )
    return resolved_root / "config" / "llvm_toolchain_releases.toml"


@lru_cache(maxsize=8)
def _load_llvm_releases_cached(
    path_text: str,
) -> LlvmReleaseManifest:
    path = Path(path_text)
    try:
        raw = path.read_bytes()
        payload = tomllib.loads(raw.decode("utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise LlvmToolchainConfigError(
            f"invalid LLVM release manifest {path}: {exc}"
        ) from exc
    if payload.get("schema_version") != 2:
        raise LlvmToolchainConfigError(
            f"unsupported LLVM release manifest schema at {path}"
        )
    default = payload.get("default_release")
    canonical_build_type = payload.get("canonical_build_type")
    debian_installer = payload.get("debian_installer")
    wasi_sysroot = payload.get("wasi_sysroot")
    rows = payload.get("releases")
    if (
        not isinstance(default, str)
        or canonical_build_type not in {"Release", "RelWithDebInfo", "Debug"}
        or not isinstance(debian_installer, dict)
        or not isinstance(wasi_sysroot, dict)
        or not isinstance(rows, dict)
    ):
        raise LlvmToolchainConfigError(f"incomplete LLVM release manifest: {path}")
    installer_url = debian_installer.get("url")
    installer_sha256 = debian_installer.get("sha256")
    if (
        not isinstance(installer_url, str)
        or not installer_url.startswith("https://")
        or not isinstance(installer_sha256, str)
        or re.fullmatch(r"[0-9a-f]{64}", installer_sha256) is None
    ):
        raise LlvmToolchainConfigError(
            f"invalid Debian LLVM installer identity in {path}"
        )
    wasi_required = (
        "version",
        "llvm_version",
        "url",
        "size",
        "sha256",
        "provenance_url",
        "archive_root",
    )
    if any(name not in wasi_sysroot for name in wasi_required):
        raise LlvmToolchainConfigError(
            f"incomplete WASI sysroot release identity in {path}"
        )
    wasi_record = {name: wasi_sysroot[name] for name in wasi_required}
    if (
        not isinstance(wasi_record["version"], str)
        or re.fullmatch(r"\d+\.\d+\+m", wasi_record["version"]) is None
        or not isinstance(wasi_record["llvm_version"], str)
        or re.fullmatch(r"\d+\.\d+\.\d+", wasi_record["llvm_version"]) is None
        or not isinstance(wasi_record["url"], str)
        or not wasi_record["url"].startswith("https://")
        or not isinstance(wasi_record["size"], int)
        or wasi_record["size"] <= 0
        or not isinstance(wasi_record["sha256"], str)
        or re.fullmatch(r"[0-9a-f]{64}", wasi_record["sha256"]) is None
        or not isinstance(wasi_record["provenance_url"], str)
        or not wasi_record["provenance_url"].startswith("https://")
        or not isinstance(wasi_record["archive_root"], str)
        or wasi_record["archive_root"] != f"wasi-sysroot-{wasi_record['version']}"
    ):
        raise LlvmToolchainConfigError(
            f"invalid WASI sysroot release identity in {path}"
        )
    wasi_record_sha256 = hashlib.sha256(
        json.dumps(wasi_record, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()
    releases: list[LlvmRelease] = []
    for version, row in rows.items():
        if not isinstance(version, str) or not isinstance(row, dict):
            raise LlvmToolchainConfigError(f"invalid LLVM release row in {path}")
        required = (
            "url",
            "size",
            "source_sha256",
            "provenance_url",
            "minimum_cmake",
        )
        if any(name not in row for name in required):
            raise LlvmToolchainConfigError(
                f"LLVM release {version} is incomplete in {path}"
            )
        url = row["url"]
        size = row["size"]
        source_sha256 = row["source_sha256"]
        provenance_url = row["provenance_url"]
        minimum_cmake = row["minimum_cmake"]
        if (
            re.fullmatch(r"\d+\.\d+\.\d+", version) is None
            or not isinstance(url, str)
            or not url.startswith("https://")
            or not isinstance(size, int)
            or size <= 0
            or not isinstance(source_sha256, str)
            or not re.fullmatch(r"[0-9a-f]{64}", source_sha256)
            or not isinstance(provenance_url, str)
            or not provenance_url.startswith("https://")
            or not isinstance(minimum_cmake, str)
            or re.fullmatch(r"\d+\.\d+\.\d+", minimum_cmake) is None
        ):
            raise LlvmToolchainConfigError(
                f"LLVM release {version} has invalid identity fields in {path}"
            )
        record = {
            "version": version,
            "url": url,
            "size": size,
            "source_sha256": source_sha256,
            "provenance_url": provenance_url,
            "minimum_cmake": minimum_cmake,
        }
        record_sha256 = hashlib.sha256(
            json.dumps(record, sort_keys=True, separators=(",", ":")).encode("utf-8")
        ).hexdigest()
        releases.append(
            LlvmRelease(
                version=version,
                url=url,
                size=size,
                source_sha256=source_sha256,
                provenance_url=provenance_url,
                minimum_cmake=minimum_cmake,
                record_sha256=record_sha256,
            )
        )
    if default not in {release.version for release in releases}:
        raise LlvmToolchainConfigError(
            f"default LLVM release {default!r} is not declared in {path}"
        )
    return LlvmReleaseManifest(
        schema_version=2,
        default_release=default,
        canonical_build_type=str(canonical_build_type),
        debian_installer=LlvmDebianInstaller(
            url=installer_url,
            sha256=installer_sha256,
        ),
        wasi_sysroot=WasiSysrootRelease(
            version=wasi_record["version"],
            llvm_version=wasi_record["llvm_version"],
            url=wasi_record["url"],
            size=wasi_record["size"],
            sha256=wasi_record["sha256"],
            provenance_url=wasi_record["provenance_url"],
            archive_root=wasi_record["archive_root"],
            record_sha256=wasi_record_sha256,
        ),
        releases=tuple(sorted(releases, key=lambda release: release.version)),
        digest=hashlib.sha256(raw).hexdigest(),
    )


def load_llvm_releases(root: Path | None = None) -> LlvmReleaseManifest:
    return _load_llvm_releases_cached(str(llvm_release_manifest_path(root)))


def llvm_release(version: str, root: Path | None = None) -> LlvmRelease | None:
    manifest = load_llvm_releases(root)
    return next(
        (release for release in manifest.releases if release.version == version), None
    )


def canonical_llvm_build_type(root: Path | None = None) -> str:
    return load_llvm_releases(root).canonical_build_type


@lru_cache(maxsize=8)
def _load_llvm_architecture_contract_cached(path_text: str) -> LlvmArchitectureContract:
    path = Path(path_text)
    raw = path.read_bytes()
    try:
        payload = tomllib.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise LlvmToolchainConfigError(
            f"invalid LLVM architecture contract {path}: {exc}"
        ) from exc
    schema_version = payload.get("schema_version")
    projects = payload.get("required_projects")
    targets = payload.get("required_targets")
    rows = payload.get("architectures")
    if schema_version != 1:
        raise LlvmToolchainConfigError(
            f"unsupported LLVM architecture contract schema {schema_version!r}: {path}"
        )
    if not isinstance(projects, list) or not all(
        isinstance(item, str) for item in projects
    ):
        raise LlvmToolchainConfigError(
            f"required_projects must be a string list: {path}"
        )
    if not isinstance(targets, list) or not all(
        isinstance(item, str) for item in targets
    ):
        raise LlvmToolchainConfigError(
            f"required_targets must be a string list: {path}"
        )
    if not isinstance(rows, list):
        raise LlvmToolchainConfigError(
            f"architectures must be an array of tables: {path}"
        )
    architectures: list[LlvmHostArchitecture] = []
    seen_aliases: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise LlvmToolchainConfigError(f"architecture row must be a table: {path}")
        required = ("id", "aliases", "llvm_target", "rust_cfg", "inkwell_feature")
        if any(name not in row for name in required):
            raise LlvmToolchainConfigError(f"architecture row is incomplete: {row!r}")
        aliases = row["aliases"]
        if (
            not isinstance(aliases, list)
            or not aliases
            or not all(isinstance(alias, str) for alias in aliases)
        ):
            raise LlvmToolchainConfigError(f"architecture aliases are invalid: {row!r}")
        normalized_aliases = tuple(alias.lower().replace("_", "-") for alias in aliases)
        duplicates = seen_aliases.intersection(normalized_aliases)
        if duplicates:
            raise LlvmToolchainConfigError(
                f"duplicate LLVM architecture aliases {sorted(duplicates)}: {path}"
            )
        seen_aliases.update(normalized_aliases)
        cranelift_feature = row.get("cranelift_feature")
        cranelift_architecture = row.get("cranelift_architecture")
        if bool(cranelift_feature) != bool(cranelift_architecture):
            raise LlvmToolchainConfigError(
                "Cranelift feature and Architecture arm must be declared together: "
                f"{row!r}"
            )
        architectures.append(
            LlvmHostArchitecture(
                id=str(row["id"]),
                aliases=normalized_aliases,
                llvm_target=str(row["llvm_target"]),
                rust_cfg=str(row["rust_cfg"]),
                inkwell_feature=str(row["inkwell_feature"]),
                cranelift_feature=(
                    str(cranelift_feature) if cranelift_feature else None
                ),
                cranelift_architecture=(
                    str(cranelift_architecture) if cranelift_architecture else None
                ),
                windows_component=(
                    str(row["windows_component"])
                    if row.get("windows_component")
                    else None
                ),
                windows_target_arch=(
                    str(row["windows_target_arch"])
                    if row.get("windows_target_arch")
                    else None
                ),
                windows_host_arch=(
                    str(row["windows_host_arch"])
                    if row.get("windows_host_arch")
                    else None
                ),
            )
        )
    return LlvmArchitectureContract(
        schema_version=1,
        required_projects=tuple(projects),
        required_targets=tuple(targets),
        architectures=tuple(architectures),
        digest=hashlib.sha256(raw).hexdigest(),
    )


def load_llvm_architecture_contract(root: Path) -> LlvmArchitectureContract:
    return _load_llvm_architecture_contract_cached(
        str(llvm_architecture_contract_path(root))
    )


def llvm_host_architecture(
    root: Path,
    machine: str,
) -> LlvmHostArchitecture | None:
    normalized = machine.lower().replace("_", "-")
    contract = load_llvm_architecture_contract(root)
    return next(
        (row for row in contract.architectures if normalized in row.aliases),
        None,
    )


def required_llvm_targets_for_host(
    root: Path, machine: str | None = None
) -> tuple[str, ...]:
    raw_machine = machine or platform.machine()
    host = llvm_host_architecture(root, raw_machine)
    if host is None:
        raise LlvmToolchainConfigError(
            f"unsupported LLVM host architecture {raw_machine!r}; add it to "
            f"{llvm_architecture_contract_path(root)} with its Cargo mapping"
        )
    contract = load_llvm_architecture_contract(root)
    return tuple(dict.fromkeys((host.llvm_target, *contract.required_targets)))


def _read_toml(path: Path) -> dict[str, Any] | None:
    try:
        with path.open("rb") as fh:
            data = tomllib.load(fh)
    except FileNotFoundError:
        return None
    except tomllib.TOMLDecodeError as exc:
        raise LlvmToolchainConfigError(f"invalid TOML in {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise LlvmToolchainConfigError(f"{path} did not parse to a TOML table")
    return data


def _dependency_table(manifest: dict[str, Any], name: str) -> dict[str, Any] | None:
    deps = manifest.get("dependencies")
    if not isinstance(deps, dict):
        return None
    dep = deps.get(name)
    if dep is None:
        return None
    if isinstance(dep, str):
        return {"version": dep}
    if isinstance(dep, dict):
        return dep
    raise LlvmToolchainConfigError(f"dependency {name!r} has unsupported shape")


def _feature_values(manifest: dict[str, Any], feature: str) -> list[str]:
    features = manifest.get("features")
    if not isinstance(features, dict):
        return []
    values = features.get(feature)
    if values is None:
        return []
    if not isinstance(values, list) or not all(isinstance(v, str) for v in values):
        raise LlvmToolchainConfigError(f"feature {feature!r} has unsupported shape")
    return values


def _require_facade_routes_llvm(root: Path) -> bool:
    facade = root / "runtime" / "molt-backend" / "Cargo.toml"
    manifest = _read_toml(facade)
    if manifest is None:
        return False
    llvm_feature = _feature_values(manifest, "llvm")
    if not llvm_feature:
        return False
    if "molt-backend-native/llvm" not in llvm_feature:
        raise LlvmToolchainConfigError(
            "runtime/molt-backend llvm feature does not enable molt-backend-native/llvm"
        )
    return True


def required_llvm_backend_pin(root: Path) -> LlvmBackendPin | None:
    root = root.resolve()
    if not _require_facade_routes_llvm(root):
        return None

    inkwell_manifest_path = root / "runtime" / "molt-backend-native" / "Cargo.toml"
    manifest = _read_toml(inkwell_manifest_path)
    if manifest is None:
        return None
    inkwell = _dependency_table(manifest, "inkwell")
    if inkwell is None:
        return None

    features = inkwell.get("features")
    if not isinstance(features, list) or not all(
        isinstance(feature, str) for feature in features
    ):
        raise LlvmToolchainConfigError(
            f"inkwell dependency in {inkwell_manifest_path} must declare features"
        )

    pins: set[tuple[int, int, str]] = set()
    for feature in features:
        match = re.fullmatch(r"llvm(\d+)-(\d+)", feature)
        if match is None:
            continue
        pins.add((int(match.group(1)), int(match.group(2)), feature))
    if not pins:
        raise LlvmToolchainConfigError(
            f"inkwell dependency in {inkwell_manifest_path} has no llvm<M>-<m> feature"
        )
    if len({(major, minor) for major, minor, _feature in pins}) != 1:
        choices = ", ".join(sorted(feature for _major, _minor, feature in pins))
        raise LlvmToolchainConfigError(
            f"inkwell dependency in {inkwell_manifest_path} has conflicting LLVM pins: "
            f"{choices}"
        )

    major, minor, feature = next(iter(pins))
    llvm_sys_version = _llvm_sys_version(manifest)
    if llvm_sys_version is not None:
        expected_prefix = str(major * 10 + minor)
        if not (
            llvm_sys_version == expected_prefix
            or llvm_sys_version.startswith(expected_prefix + ".")
        ):
            raise LlvmToolchainConfigError(
                "llvm-sys version "
                f"{llvm_sys_version!r} in {inkwell_manifest_path} does not match "
                f"inkwell feature {feature!r}"
            )
    return LlvmBackendPin(
        major=major,
        minor=minor,
        inkwell_feature=feature,
        inkwell_manifest=str(inkwell_manifest_path),
        llvm_sys_version=llvm_sys_version,
    )


def _llvm_sys_version(manifest: dict[str, Any]) -> str | None:
    llvm_sys = _dependency_table(manifest, "llvm-sys")
    if llvm_sys is None:
        return None
    version = llvm_sys.get("version")
    if version is None:
        return None
    if not isinstance(version, str):
        raise LlvmToolchainConfigError("llvm-sys version must be a string")
    return version


def required_llvm_backend_major(root: Path) -> int | None:
    pin = required_llvm_backend_pin(root)
    return None if pin is None else pin.major


def llvm_sys_prefix_env_var(major: int, minor: int = 1) -> str:
    return f"LLVM_SYS_{major * 10 + minor}_PREFIX"


def llvm_sys_prefix_env_var_for_version(version: str) -> str:
    parts = version.split(".")
    if len(parts) < 2:
        raise LlvmToolchainConfigError(
            f"LLVM version must include major.minor: {version}"
        )
    try:
        major = int(parts[0])
        minor = int(parts[1])
    except ValueError as exc:
        raise LlvmToolchainConfigError(
            f"LLVM version must start with numeric major.minor: {version}"
        ) from exc
    return llvm_sys_prefix_env_var(major, minor)


def default_llvm_release(major: int, minor: int = 1) -> str:
    manifest = load_llvm_releases()
    selected = next(
        (
            release.version
            for release in manifest.releases
            if release.version.split(".")[:2] == [str(major), str(minor)]
            and release.version == manifest.default_release
        ),
        None,
    )
    if selected is None:
        raise LlvmToolchainConfigError(
            f"no canonical LLVM release is declared for {major}.{minor}"
        )
    return selected


def mlir_sys_prefix_env_var(major: int) -> str:
    """Return the environment authority consumed by ``mlir-sys``."""

    return f"MLIR_SYS_{major * 10}_PREFIX"


def tablegen_prefix_env_var(major: int) -> str:
    """Return the environment authority consumed by ``tblgen``."""

    return f"TABLEGEN_{major * 10}_PREFIX"


def managed_llvm_prefix(root: Path, pin: LlvmBackendPin | None = None) -> Path:
    """Return Molt's content-versioned managed LLVM/MLIR installation root."""

    resolved_pin = pin if pin is not None else required_llvm_backend_pin(root)
    if resolved_pin is None:
        raise LlvmToolchainConfigError(
            f"could not resolve LLVM backend feature pin under {root}"
        )
    from molt.dx import canonical_toolchain_root

    managed = (
        canonical_toolchain_root(root, require_exists=False)
        / "toolchains"
        / f"llvm-{resolved_pin.default_release}"
    )
    reject_poison_toolchain_path(managed, authority="managed LLVM prefix")
    return managed


def managed_llvm_paths(
    root: Path,
    pin: LlvmBackendPin | None = None,
    *,
    version: str | None = None,
) -> LlvmManagedPaths:
    """Return the one durable source/build/download/install custody family."""

    resolved_pin = pin if pin is not None else required_llvm_backend_pin(root)
    if resolved_pin is None:
        raise LlvmToolchainConfigError(
            f"could not resolve LLVM backend feature pin under {root}"
        )
    release = version or resolved_pin.default_release
    from molt.dx import canonical_toolchain_root

    custody = canonical_toolchain_root(root, require_exists=False) / "toolchains"
    reject_poison_toolchain_path(custody, authority="managed LLVM custody")
    return LlvmManagedPaths(
        root=custody,
        prefix=custody / f"llvm-{release}",
        archive=custody / "downloads" / f"llvm-project-{release}.tar.xz",
        source_root=custody / "sources" / f"llvm-project-{release}",
        build_dir=custody / "build" / f"llvm-{release}",
    )


def llvm_bootstrap_command(pin: LlvmBackendPin, *, python: str = "python") -> str:
    """Render the single user-facing bootstrap command authority."""

    return f"{python} -m tools.bootstrap_llvm --version {pin.default_release}"


def llvm_debian_dev_packages(root: Path, major: int) -> tuple[str, ...]:
    """Project the manifest-owned SDK components into apt.llvm.org package names."""

    projects = set(load_llvm_architecture_contract(root).required_projects)
    packages = {f"llvm-{major}-dev"}
    if "clang" in projects:
        packages.update((f"clang-{major}", f"libclang-{major}-dev"))
    if "lld" in projects:
        packages.update((f"lld-{major}", f"liblld-{major}-dev"))
    if "mlir" in projects:
        packages.update((f"libmlir-{major}-dev", f"mlir-{major}-tools"))
    if "polly" in projects:
        packages.add(f"libpolly-{major}-dev")
    return tuple(sorted(packages))


def llvm_config_executable(prefix: Path) -> Path:
    """Return the conventional in-prefix ``llvm-config`` location.

    Discovery of system/package-manager layouts belongs to
    :func:`discover_llvm_toolchain`; this helper intentionally remains useful
    for managed prefixes whose tools are always self-contained.
    """

    bin_dir = prefix / "bin"
    windows = bin_dir / "llvm-config.exe"
    return windows if windows.is_file() else bin_dir / "llvm-config"


def llvm_config_names(pin: LlvmBackendPin) -> tuple[str, ...]:
    """Return the complete cross-platform executable-name family in priority order."""

    stems = (f"llvm-config-{pin.major}", f"llvm-config{pin.major}", "llvm-config")
    if os.name != "nt":
        return stems
    return tuple(f"{stem}.exe" for stem in stems) + stems


def _llvm_config_prefix(executable: Path) -> Path:
    rendered = _run_llvm_config(executable, "--prefix")
    if not rendered:
        raise LlvmToolchainConfigError(
            f"llvm-config returned an empty SDK prefix: {executable}"
        )
    reject_poison_toolchain_path(rendered, authority=f"{executable} --prefix")
    return Path(rendered).expanduser().resolve(strict=False)


def _path_identity(path: Path) -> str:
    return os.path.normcase(os.path.normpath(str(path.resolve(strict=False))))


def _dedupe_paths(paths: list[tuple[Path, str]]) -> tuple[tuple[Path, str], ...]:
    seen: set[str] = set()
    result: list[tuple[Path, str]] = []
    for path, source in paths:
        identity = _path_identity(path)
        if identity not in seen:
            seen.add(identity)
            result.append((path, source))
    return tuple(result)


def _llvm_config_candidates(
    root: Path,
    pin: LlvmBackendPin,
    env: dict[str, str],
    explicit_prefix: Path | None,
    llvm_sys_search_prefix: Path | None,
) -> tuple[tuple[Path, str], ...]:
    candidates: list[tuple[Path, str]] = []
    if configured := env.get("LLVM_CONFIG_PATH", "").strip():
        candidates.append((Path(configured).expanduser(), "LLVM_CONFIG_PATH"))

    prefixes = [explicit_prefix] if explicit_prefix is not None else []
    if llvm_sys_search_prefix is not None:
        prefixes.append(llvm_sys_search_prefix)
    if explicit_prefix is None:
        prefixes.append(managed_llvm_prefix(root, pin))
    for prefix in prefixes:
        assert prefix is not None
        for name in llvm_config_names(pin):
            candidates.append((prefix / "bin" / name, f"prefix:{prefix}"))

    path_value = env.get("PATH", "")
    for name in llvm_config_names(pin):
        if discovered := shutil.which(name, path=path_value):
            candidates.append((Path(discovered), "PATH"))

    if platform.system() == "Darwin":
        for homebrew_root in (Path("/opt/homebrew"), Path("/usr/local")):
            prefix = homebrew_root / "opt" / f"llvm@{pin.major}"
            for name in llvm_config_names(pin):
                candidates.append((prefix / "bin" / name, "Homebrew"))
    return _dedupe_paths(candidates)


def discover_llvm_toolchain(
    root: Path,
    *,
    environ: dict[str, str] | None = None,
) -> LlvmToolchainDiscovery | None:
    """Discover one version-correct LLVM executable/prefix identity.

    Molt/MLIR/TableGen prefixes identify the SDK. ``LLVM_SYS_*_PREFIX`` is the
    llvm-sys executable-search root (the directory containing ``bin``), which
    may differ for a package-manager split layout. Candidate executables are
    accepted only when their exact version and reported ``--prefix`` bind both
    path shapes to one SDK identity.
    """

    pin = required_llvm_backend_pin(root)
    if pin is None:
        return None
    env = dict(os.environ if environ is None else environ)
    sdk_authority_names = (
        "MOLT_LLVM_PREFIX",
        mlir_sys_prefix_env_var(pin.major),
        tablegen_prefix_env_var(pin.major),
    )
    for name in (*sdk_authority_names, pin.env_var):
        if value := env.get(name, "").strip():
            reject_poison_toolchain_path(value, authority=name)
    if llvm_config_path := env.get("LLVM_CONFIG_PATH", "").strip():
        reject_poison_toolchain_path(llvm_config_path, authority="LLVM_CONFIG_PATH")
    if target_root := env.get("MOLT_TARGET_ROOT", "").strip():
        reject_poison_toolchain_path(target_root, authority="MOLT_TARGET_ROOT")

    explicit = {
        _normalized_prefix(value)
        for name in sdk_authority_names
        if (value := env.get(name, "").strip())
    }
    if len(explicit) > 1:
        rendered = ", ".join(str(path) for path in sorted(explicit, key=str))
        raise LlvmToolchainConfigError(
            "LLVM/MLIR prefix authorities disagree; configure one toolchain family: "
            f"{rendered}"
        )
    explicit_prefix = next(iter(explicit), None)
    llvm_sys_search_prefix = (
        _normalized_prefix(value)
        if (value := env.get(pin.env_var, "").strip())
        else None
    )
    rejected: list[str] = []
    for candidate, source in _llvm_config_candidates(
        root, pin, env, explicit_prefix, llvm_sys_search_prefix
    ):
        reject_poison_toolchain_path(candidate, authority=f"{source} llvm-config")
        if not candidate.is_file():
            continue
        try:
            resolved_candidate = candidate.resolve()
            _major, _minor, version = _llvm_config_version(str(resolved_candidate))
            reported_prefix = _llvm_config_prefix(resolved_candidate)
        except LlvmToolchainConfigError as exc:
            rejected.append(str(exc))
            continue
        if version != pin.default_release:
            rejected.append(
                f"{resolved_candidate} reports LLVM {version}; expected exactly {pin.default_release}"
            )
            continue
        if explicit_prefix is not None and _path_identity(
            reported_prefix
        ) != _path_identity(explicit_prefix):
            rejected.append(
                f"{resolved_candidate} reports prefix {reported_prefix}, not configured prefix {explicit_prefix}"
            )
            continue
        if llvm_sys_search_prefix is not None and _path_identity(
            llvm_sys_search_prefix
        ) not in {
            _path_identity(reported_prefix),
            _path_identity(resolved_candidate.parent.parent),
        }:
            rejected.append(
                f"{pin.env_var}={llvm_sys_search_prefix} is neither the SDK prefix "
                f"{reported_prefix} nor the llvm-config search prefix "
                f"{resolved_candidate.parent.parent}"
            )
            continue
        return LlvmToolchainDiscovery(
            prefix=reported_prefix,
            llvm_config=resolved_candidate,
            version=version,
            source=source,
        )

    if (
        explicit_prefix is not None
        or llvm_sys_search_prefix is not None
        or env.get("LLVM_CONFIG_PATH", "").strip()
        or rejected
    ):
        detail = "; ".join(rejected[-4:]) or "no candidate executable exists"
        raise LlvmToolchainConfigError(
            "LLVM/MLIR authority has no matching llvm-config: " + detail
        )
    return None


def _run_llvm_config(executable: Path, *args: str) -> str:
    try:
        result = subprocess.run(
            [str(executable), *args],
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise LlvmToolchainConfigError(
            f"could not query LLVM toolchain from {executable}: {exc}"
        ) from exc
    return result.stdout.strip()


def _required_tool(prefix: Path, *names: str) -> Path:
    for name in names:
        candidate = prefix / "bin" / name
        if candidate.is_file():
            return candidate
    rendered = ", ".join(str(prefix / "bin" / name) for name in names)
    raise LlvmToolchainConfigError(
        f"LLVM/MLIR prefix is missing required tool: {rendered}"
    )


def _required_directory(prefix: Path, relative: str) -> Path:
    candidate = prefix / relative
    if not candidate.is_dir():
        raise LlvmToolchainConfigError(
            f"LLVM/MLIR prefix is missing required directory: {candidate}"
        )
    return candidate


def _required_file(prefix: Path, relative: str) -> Path:
    candidate = prefix / relative
    if not candidate.is_file():
        raise LlvmToolchainConfigError(
            f"LLVM/MLIR prefix is missing required SDK file: {candidate}"
        )
    return candidate


def _clang_resource_header(prefix: Path, major: int) -> Path:
    candidates = tuple(sorted((prefix / "lib" / "clang").glob("*/include/stddef.h")))
    matching = tuple(
        path for path in candidates if path.parents[1].name.split(".")[0] == str(major)
    )
    if len(matching) != 1:
        raise LlvmToolchainConfigError(
            "LLVM/MLIR prefix must contain exactly one matching Clang resource "
            f"header for LLVM {major}: {list(matching)}"
        )
    return matching[0]


def _llvm_config_tokens(output: str) -> tuple[str, ...]:
    tokens = shlex.split(output, posix=os.name != "nt")
    if os.name != "nt":
        return tuple(tokens)
    return tuple(
        token[1:-1]
        if len(token) >= 2 and token[0] == token[-1] and token[0] in {'"', "'"}
        else token
        for token in tokens
    )


def _llvm_link_closure(
    prefix: Path,
    llvm_config: Path,
) -> tuple[tuple[str, ...], tuple[Path, ...]]:
    local_output = _run_llvm_config(
        llvm_config, "--link-static", "--libs", "core", "support"
    )
    system_output = _run_llvm_config(llvm_config, "--system-libs")
    lib_dir = prefix / "lib"
    resolved_local: list[Path] = []
    rendered: list[str] = []
    for token in _llvm_config_tokens(local_output):
        if token.startswith("-l"):
            stem = token[2:]
            candidates = (
                lib_dir / f"{stem}.lib",
                lib_dir / f"lib{stem}.lib",
                lib_dir / f"lib{stem}.a",
                lib_dir / f"lib{stem}.so",
                lib_dir / f"lib{stem}.dylib",
            )
            path = next(
                (candidate for candidate in candidates if candidate.is_file()), None
            )
            if path is None:
                raise LlvmToolchainConfigError(
                    f"llvm-config link closure names missing library {token} in {lib_dir}"
                )
        else:
            candidate = Path(token)
            path = candidate if candidate.is_absolute() else lib_dir / candidate
            if not path.is_file():
                raise LlvmToolchainConfigError(
                    f"llvm-config link closure names missing library: {path}"
                )
        resolved = path.resolve()
        try:
            relative = resolved.relative_to(prefix)
        except ValueError as exc:
            raise LlvmToolchainConfigError(
                f"llvm-config link closure escapes the verified prefix: {resolved}"
            ) from exc
        resolved_local.append(resolved)
        rendered.append(str(relative).replace("\\", "/"))
    rendered.extend(f"system:{token}" for token in _llvm_config_tokens(system_output))
    if not resolved_local:
        raise LlvmToolchainConfigError(
            "llvm-config returned an empty static link closure"
        )
    return tuple(rendered), tuple(resolved_local)


def _compile_link_probe(
    prefix: Path,
    clangxx: Path,
    host_linker: Path,
    link_closure: tuple[str, ...],
) -> tuple[str, ...]:
    """Compile and link the SDK headers and authoritative llvm-config closure."""

    local_libraries = [
        str(prefix / token) for token in link_closure if not token.startswith("system:")
    ]
    system_libraries = [
        (
            f"-l{token.removeprefix('system:')[:-4]}"
            if os.name == "nt"
            and token.removeprefix("system:").lower().endswith(".lib")
            and not Path(token.removeprefix("system:")).is_absolute()
            else token.removeprefix("system:")
        )
        for token in link_closure
        if token.startswith("system:")
    ]
    with tempfile.TemporaryDirectory(prefix="molt-llvm-sdk-probe-") as temp:
        temp_root = Path(temp)
        source = temp_root / "sdk_probe.cpp"
        output = temp_root / ("sdk_probe.exe" if os.name == "nt" else "sdk_probe")
        source.write_text(
            "#include <llvm/IR/LLVMContext.h>\n"
            "#include <llvm-c/Core.h>\n"
            "#include <clang/Basic/Version.h>\n"
            "#include <lld/Common/Driver.h>\n"
            "#include <mlir/IR/MLIRContext.h>\n"
            "#include <mlir-c/IR.h>\n"
            "#include <stddef.h>\n"
            "int main() { llvm::LLVMContext context; return 0; }\n",
            encoding="utf-8",
        )
        command = [
            str(clangxx),
            "-std=c++17",
            f"--ld-path={host_linker}",
            *(["-fms-runtime-lib=dll"] if os.name == "nt" else []),
            f"-I{prefix / 'include'}",
            str(source),
            *local_libraries,
            *system_libraries,
            "-o",
            str(output),
        ]
        try:
            result = subprocess.run(
                command,
                check=False,
                capture_output=True,
                text=True,
                timeout=120,
            )
        except (OSError, subprocess.SubprocessError) as exc:
            raise LlvmToolchainConfigError(
                f"LLVM/MLIR SDK compile-link probe could not run: {exc}"
            ) from exc
        if result.returncode != 0:
            diagnostic = "\n".join(
                part.strip() for part in (result.stdout, result.stderr) if part.strip()
            )
            raise LlvmToolchainConfigError(
                "LLVM/MLIR SDK headers and llvm-config link closure do not "
                f"compile and link together: {diagnostic}"
            )
    return _link_probe_identity(prefix, host_linker, link_closure)


def _link_probe_identity(
    prefix: Path,
    host_linker: Path,
    link_closure: tuple[str, ...],
) -> tuple[str, ...]:
    entrypoint = executable_entrypoint_name(host_linker)
    if not is_llvm_linker_role(entrypoint) or entrypoint == "wasm-ld":
        raise LlvmToolchainConfigError(
            "verified host linker must select ld.lld, ld64.lld, or lld-link; "
            f"generic lld is not a host linker role: {host_linker}"
        )
    resolved_prefix = prefix.resolve()
    try:
        host_linker.resolve().relative_to(resolved_prefix)
        linker_identity = str(
            lexical_executable_path(host_linker).relative_to(
                lexical_executable_path(prefix)
            )
        ).replace("\\", "/")
    except ValueError as exc:
        raise LlvmToolchainConfigError(
            f"verified host linker escapes the LLVM prefix: {host_linker}"
        ) from exc
    return ("language:c++17", f"linker:{linker_identity}", *link_closure)


_LLVM_TOOL_VERSION_RE = re.compile(
    r"\b(?:clang\s+version|LLD|LLVM\s+version)\s+"
    r"(?P<major>\d+)\.(?P<minor>\d+)(?:\.(?P<patch>\d+))?\b",
    re.IGNORECASE,
)
_PLAIN_LLVM_VERSION_RE = re.compile(
    r"^\s*(?P<major>\d+)\.(?P<minor>\d+)(?:\.(?P<patch>\d+))?\b"
)


def _sha256_file(path: Path) -> str:
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def _content_paths(
    prefix: Path,
    required_libraries: tuple[Path, ...],
) -> tuple[Path, ...]:
    headers: set[Path] = set()
    headers.update(path for path in (prefix / "include").rglob("*") if path.is_file())
    clang_resource_root = prefix / "lib" / "clang"
    if clang_resource_root.is_dir():
        headers.update(
            path
            for resource_include in clang_resource_root.glob("*/include")
            for path in resource_include.rglob("*")
            if path.is_file()
        )
    return tuple(sorted(headers.union(required_libraries)))


def _content_manifest(
    prefix: Path,
    paths: tuple[Path, ...],
    *,
    hash_contents: bool,
) -> tuple[tuple[LlvmContentFact, ...], str | None]:
    metadata: list[tuple[Path, os.stat_result, str, int | None]] = []
    for path in paths:
        stat = path.stat()
        relative = str(path.relative_to(prefix)).replace("\\", "/")
        change_ns = _content_change_time_ns(path, stat)
        metadata.append((path, stat, relative, change_ns))

    digests: dict[Path, str] = {}
    if hash_contents:
        hash_paths = tuple(path for path, _stat, _relative, _change_ns in metadata)
        workers = min(8, max(1, len(hash_paths)))
        with ThreadPoolExecutor(max_workers=workers) as executor:
            hashes = executor.map(_sha256_file, hash_paths)
            digests = {
                path: digest for path, digest in zip(hash_paths, hashes, strict=True)
            }

    facts: list[LlvmContentFact] = []
    aggregate = hashlib.sha256() if hash_contents else None
    for path, stat, relative, change_ns in metadata:
        sha256 = digests.get(path, "")
        if hash_contents:
            current = path.stat()
            current_change_ns = _content_change_time_ns(path, current)
            if (
                current.st_size != stat.st_size
                or current.st_mtime_ns != stat.st_mtime_ns
                or current_change_ns != change_ns
            ):
                raise LlvmToolchainConfigError(
                    f"LLVM/MLIR content changed while it was being attested: {path}"
                )
        fact = LlvmContentFact(
            path=relative,
            size=stat.st_size,
            mtime_ns=stat.st_mtime_ns,
            change_ns=change_ns,
            sha256=sha256,
        )
        facts.append(fact)
        if aggregate is not None:
            aggregate.update(relative.encode("utf-8"))
            aggregate.update(b"\0")
            aggregate.update(str(stat.st_size).encode("ascii"))
            aggregate.update(b"\0")
            aggregate.update(sha256.encode("ascii"))
            aggregate.update(b"\n")
    return tuple(facts), aggregate.hexdigest() if aggregate is not None else None


def _tool_version_fact(
    prefix: Path,
    role: str,
    path: Path,
    *,
    expected_version: str,
    exact_version: bool,
) -> LlvmToolVersionFact:
    if is_llvm_linker_role(role) and not executable_selects_linker_role(path, role):
        raise LlvmToolchainConfigError(
            f"required LLVM linker role {role} cannot use entrypoint {path}"
        )
    try:
        result = subprocess.run(
            [str(path), "--version"],
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise LlvmToolchainConfigError(
            f"could not query required LLVM tool {role} at {path}: {exc}"
        ) from exc
    output = "\n".join(part for part in (result.stdout, result.stderr) if part).strip()
    match = (
        _PLAIN_LLVM_VERSION_RE.search(output)
        if role == "llvm-config"
        else _LLVM_TOOL_VERSION_RE.search(output)
    )
    if result.returncode != 0 or match is None:
        raise LlvmToolchainConfigError(
            f"required LLVM tool {role} did not report an LLVM version: {path}: {output!r}"
        )
    version = ".".join(
        (match.group("major"), match.group("minor"), match.group("patch") or "0")
    )
    version_matches = (
        version == expected_version
        if exact_version
        else version == expected_version or version.startswith(expected_version + ".")
    )
    if not version_matches:
        raise LlvmToolchainConfigError(
            f"required LLVM tool {role} reports {version}; expected "
            f"{'exactly ' + expected_version if exact_version else expected_version + '.x'}: "
            f"{path}"
        )
    stat = path.stat()
    sha256 = _sha256_file(path)
    try:
        identity = str(path.relative_to(prefix)).replace("\\", "/")
    except ValueError:
        identity = f"external:{path}"
    return LlvmToolVersionFact(
        role=role,
        path=identity,
        version=version,
        size=stat.st_size,
        sha256=sha256,
    )


def llvm_attestation_path(prefix: Path) -> Path:
    return prefix / LLVM_ATTESTATION_FILENAME


def _llvm_attestation_custody(
    root: Path,
    prefix: Path,
    release: LlvmRelease | None,
) -> str:
    if release is None:
        return "development-noncanonical"
    managed = managed_llvm_paths(root, version=release.version).prefix.resolve()
    return (
        "canonical-managed-release"
        if prefix.expanduser().resolve() == managed
        else "manifest-release-noncanonical-prefix"
    )


def _attestation_value_summary(value: Any) -> str:
    if isinstance(value, (list, dict)):
        encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
        return f"{type(value).__name__}(items={len(value)}, sha256={hashlib.sha256(encoded).hexdigest()})"
    return repr(value)


def verify_llvm_toolchain_prefix(
    root: Path,
    prefix: Path,
    *,
    version: str | None = None,
    expected_targets: tuple[str, ...] | None = None,
    require_attestation: bool = False,
    llvm_config_override: Path | None = None,
    content_policy: Literal["cached", "full"] = "cached",
) -> LlvmPrefixVerification:
    """Verify the complete compiler/linker/MLIR prefix consumed by Molt."""

    reject_poison_toolchain_path(prefix, authority="LLVM/MLIR prefix")
    resolved = prefix.expanduser().resolve()
    pin = required_llvm_backend_pin(root)
    if pin is None:
        raise LlvmToolchainConfigError(
            f"could not resolve LLVM backend feature pin under {root}"
        )
    contract = load_llvm_architecture_contract(root)
    llvm_config = (
        llvm_config_override.expanduser().resolve()
        if llvm_config_override is not None
        else llvm_config_executable(resolved)
    )
    if not llvm_config.is_file():
        raise LlvmToolchainConfigError(
            f"LLVM/MLIR prefix does not contain llvm-config: {llvm_config}"
        )
    actual_version = _run_llvm_config(llvm_config, "--version")
    expected_version = version or pin.default_release
    release = llvm_release(expected_version, root)
    managed = managed_llvm_paths(root, pin, version=expected_version).prefix.resolve()
    canonical_contract = resolved == managed or require_attestation
    if canonical_contract and release is None:
        raise LlvmToolchainConfigError(
            f"canonical LLVM/MLIR prefix requires a pinned release: {expected_version}"
        )
    if actual_version != expected_version:
        raise LlvmToolchainConfigError(
            "LLVM/MLIR toolchain version does not match the manifest authority: "
            f"expected exactly {expected_version}, found {actual_version} at {llvm_config}"
        )
    built_targets = tuple(
        sorted(_run_llvm_config(llvm_config, "--targets-built").split())
    )
    required_targets = set(expected_targets or contract.required_targets)
    missing_targets = sorted(required_targets.difference(built_targets))
    if missing_targets:
        raise LlvmToolchainConfigError(
            f"LLVM/MLIR prefix is missing required targets {missing_targets}; "
            f"built targets are {list(built_targets)}"
        )
    suffix = ".exe" if os.name == "nt" else ""
    host_linker_role = host_llvm_linker_role(platform.system())
    host_linker = (f"{host_linker_role}{suffix}",)
    tools = (
        ("llvm-config", llvm_config),
        ("clang", _required_tool(resolved, f"clang{suffix}")),
        ("clang++", _required_tool(resolved, f"clang++{suffix}")),
        (
            host_linker_role,
            _required_tool(resolved, *host_linker),
        ),
        ("wasm-ld", _required_tool(resolved, f"wasm-ld{suffix}")),
        ("mlir-opt", _required_tool(resolved, f"mlir-opt{suffix}")),
        ("mlir-tblgen", _required_tool(resolved, f"mlir-tblgen{suffix}")),
        ("llvm-tblgen", _required_tool(resolved, f"llvm-tblgen{suffix}")),
    )
    directories = (
        _required_directory(resolved, "include/llvm"),
        _required_directory(resolved, "include/mlir"),
        _required_directory(resolved, "include/mlir-c"),
        _required_directory(resolved, "lib"),
    )
    sdk_sentinels = (
        _required_file(resolved, "include/llvm/IR/LLVMContext.h"),
        _required_file(resolved, "include/llvm-c/Core.h"),
        _required_file(resolved, "include/clang/Basic/Version.h"),
        _required_file(resolved, "include/lld/Common/Driver.h"),
        _required_file(resolved, "include/mlir/IR/MLIRContext.h"),
        _required_file(resolved, "include/mlir-c/IR.h"),
        _clang_resource_header(resolved, int(expected_version.split(".")[0])),
    )
    link_closure, link_libraries = _llvm_link_closure(resolved, llvm_config)
    host_linker_path = tools[3][1]
    link_probe = _link_probe_identity(resolved, host_linker_path, link_closure)
    if not (require_attestation and content_policy == "cached"):
        link_probe = _compile_link_probe(
            resolved,
            tools[2][1],
            host_linker_path,
            link_closure,
        )
    lib_dir = resolved / "lib"
    library_files = tuple(path for path in lib_dir.iterdir() if path.is_file())

    def library_family(name: str) -> tuple[Path, ...]:
        family = tuple(
            sorted(path for path in library_files if name in path.name.lower())
        )
        if not family:
            raise LlvmToolchainConfigError(
                f"{name.upper()} libraries are missing from {lib_dir}"
            )
        return family

    library_families = tuple(
        library_family(name) for name in ("llvm", "mlir", "polly", "lld")
    )
    required_libraries = tuple(
        sorted({path for family in library_families for path in family})
    )
    library_facts = tuple(
        LlvmLibraryFact(
            path=str(path.relative_to(resolved)).replace("\\", "/"),
            size=path.stat().st_size,
            mtime_ns=path.stat().st_mtime_ns,
        )
        for path in required_libraries
    )
    content_paths = _content_paths(resolved, required_libraries)
    content_facts, content_digest = _content_manifest(
        resolved,
        content_paths,
        hash_contents=content_policy == "full",
    )

    def inspect_tool(item: tuple[str, Path]) -> LlvmToolVersionFact:
        role, path = item
        return _tool_version_fact(
            resolved,
            role,
            path,
            expected_version=expected_version,
            exact_version=True,
        )

    with ThreadPoolExecutor(max_workers=len(tools)) as executor:
        tool_versions = tuple(executor.map(inspect_tool, tools))

    def asset_identity(path: Path) -> str:
        try:
            return str(path.relative_to(resolved)).replace("\\", "/")
        except ValueError:
            return f"external:{path}"

    assets = tuple(
        sorted(
            asset_identity(path)
            for path in (
                *(path for _role, path in tools),
                *directories,
                *sdk_sentinels,
                *required_libraries,
                *link_libraries,
            )
        )
    )
    if require_attestation:
        path = llvm_attestation_path(resolved)
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise LlvmToolchainConfigError(
                f"managed LLVM/MLIR prefix lacks a valid attestation: {path}: {exc}"
            ) from exc
        expected = {
            "schema": LLVM_ATTESTATION_SCHEMA,
            "architecture_contract_sha256": contract.digest,
            "version": actual_version,
            "prefix": str(resolved),
            "llvm_config": str(llvm_config),
            "targets": list(built_targets),
            "assets": list(assets),
            "tool_versions": [asdict(fact) for fact in tool_versions],
            "library_facts": [asdict(fact) for fact in library_facts],
            "link_closure": list(link_closure),
            "link_probe": list(link_probe),
            "release": asdict(release) if release is not None else None,
            "custody": _llvm_attestation_custody(root, resolved, release),
            "release_manifest_sha256": load_llvm_releases(root).digest,
            "build_config": {
                "projects": sorted(contract.required_projects),
                "targets": list(built_targets),
                "build_type": canonical_llvm_build_type(root),
            },
        }
        mismatches = {
            key: (payload.get(key), value)
            for key, value in expected.items()
            if payload.get(key) != value
        }
        if mismatches:
            details = "; ".join(
                f"{key}: attested={_attestation_value_summary(attested)} "
                f"live={_attestation_value_summary(live)}"
                for key, (attested, live) in mismatches.items()
            )
            raise LlvmToolchainConfigError(
                f"managed LLVM/MLIR attestation drift at {path}: {details}"
            )
        attested_content = payload.get("content_facts")
        attested_digest = payload.get("content_digest")
        if not isinstance(attested_content, list) or not isinstance(
            attested_digest, str
        ):
            raise LlvmToolchainConfigError(
                f"managed LLVM/MLIR attestation lacks content proof: {path}"
            )
        live_metadata = [
            {
                "path": fact.path,
                "size": fact.size,
                "mtime_ns": fact.mtime_ns,
                "change_ns": fact.change_ns,
            }
            for fact in content_facts
        ]
        attested_metadata = [
            {key: fact.get(key) for key in ("path", "size", "mtime_ns", "change_ns")}
            for fact in attested_content
            if isinstance(fact, dict)
        ]
        metadata_is_trusted = os.name != "nt" or all(
            fact.change_ns is not None for fact in content_facts
        )
        content_metadata_matches = (
            metadata_is_trusted and live_metadata == attested_metadata
        )
        if content_policy == "full" or not content_metadata_matches:
            if content_digest is None:
                content_facts, content_digest = _content_manifest(
                    resolved,
                    content_paths,
                    hash_contents=True,
                )
            if content_digest != attested_digest:
                raise LlvmToolchainConfigError(
                    f"managed LLVM/MLIR content digest drift at {path}: "
                    f"live={content_digest} attested={attested_digest}"
                )
            if [asdict(fact) for fact in content_facts] != attested_content:
                raise LlvmToolchainConfigError(
                    f"managed LLVM/MLIR content metadata drift at {path}"
                )
        attested_projects = set(payload.get("projects", ()))
        required_projects = set(contract.required_projects)
        if not required_projects.issubset(attested_projects):
            raise LlvmToolchainConfigError(
                "managed LLVM/MLIR attestation omits projects "
                f"{sorted(required_projects - attested_projects)}"
            )
    return LlvmPrefixVerification(
        prefix=resolved,
        llvm_config=llvm_config,
        version=actual_version,
        targets=built_targets,
        assets=assets,
        tool_versions=tool_versions,
        library_facts=library_facts,
        content_facts=content_facts,
        content_digest=content_digest,
        link_closure=link_closure,
        link_probe=link_probe,
        release=release,
    )


def write_llvm_toolchain_attestation(
    root: Path,
    verification: LlvmPrefixVerification,
    *,
    projects: tuple[str, ...],
    build_type: str | None = None,
    published_prefix: Path | None = None,
) -> Path:
    """Atomically publish proof for a completely verified managed prefix."""

    contract = load_llvm_architecture_contract(root)
    missing_projects = set(contract.required_projects).difference(projects)
    if missing_projects:
        raise LlvmToolchainConfigError(
            f"cannot attest incomplete LLVM/MLIR prefix; missing projects {sorted(missing_projects)}"
        )
    release = verification.release
    if release is not None:
        mismatched_versions = {
            fact.role: fact.version
            for fact in verification.tool_versions
            if fact.version != release.version
        }
        if verification.version != release.version or mismatched_versions:
            raise LlvmToolchainConfigError(
                "cannot bind a canonical LLVM release record to mixed patch versions: "
                f"release={release.version} llvm-config={verification.version} "
                f"tools={mismatched_versions}"
            )
    content_facts = verification.content_facts
    content_digest = verification.content_digest
    required_libraries = tuple(
        verification.prefix / fact.path for fact in verification.library_facts
    )
    content_paths = _content_paths(verification.prefix, required_libraries)
    rehashed_for_publication = False
    if content_digest is None or any(not fact.sha256 for fact in content_facts):
        content_facts, content_digest = _content_manifest(
            verification.prefix,
            content_paths,
            hash_contents=True,
        )
        rehashed_for_publication = True
    if content_digest is None:
        raise LlvmToolchainConfigError(
            "cannot attest LLVM/MLIR prefix without a complete content digest"
        )
    if not rehashed_for_publication:
        current_metadata, _unused = _content_manifest(
            verification.prefix,
            content_paths,
            hash_contents=False,
        )

        def metadata(facts: tuple[LlvmContentFact, ...]) -> list[dict[str, object]]:
            return [
                {
                    "path": fact.path,
                    "size": fact.size,
                    "mtime_ns": fact.mtime_ns,
                    "change_ns": fact.change_ns,
                }
                for fact in facts
            ]

        metadata_is_trusted = os.name != "nt" or all(
            fact.change_ns is not None for fact in current_metadata
        )
        if not metadata_is_trusted or metadata(current_metadata) != metadata(
            content_facts
        ):
            current_facts, current_digest = _content_manifest(
                verification.prefix,
                content_paths,
                hash_contents=True,
            )
            if current_digest != content_digest or [
                asdict(fact) for fact in current_facts
            ] != [asdict(fact) for fact in content_facts]:
                raise LlvmToolchainConfigError(
                    "LLVM/MLIR content changed before attestation publication"
                )
    for fact in verification.tool_versions:
        tool = verification.prefix / fact.path
        stat = tool.stat()
        digest = _sha256_file(tool)
        if stat.st_size != fact.size or digest != fact.sha256:
            raise LlvmToolchainConfigError(
                f"required LLVM tool changed before attestation publication: {tool}"
            )
    content_by_path = {fact.path: fact for fact in content_facts}
    for fact in verification.library_facts:
        content = content_by_path.get(fact.path)
        if (
            content is None
            or content.size != fact.size
            or content.mtime_ns != fact.mtime_ns
        ):
            raise LlvmToolchainConfigError(
                "required LLVM/MLIR library changed before attestation publication: "
                f"{verification.prefix / fact.path}"
            )
    attested_prefix = (
        published_prefix.expanduser().resolve()
        if published_prefix is not None
        else verification.prefix
    )
    resolved_build_type = build_type or canonical_llvm_build_type(root)
    custody = _llvm_attestation_custody(root, attested_prefix, release)
    if custody == "canonical-managed-release":
        expected_projects = set(contract.required_projects)
        expected_targets = set(required_llvm_targets_for_host(root))
        if set(projects) != expected_projects:
            raise LlvmToolchainConfigError(
                "canonical LLVM project set must match exactly: "
                f"expected={sorted(expected_projects)} found={sorted(projects)}"
            )
        missing_targets = sorted(expected_targets.difference(verification.targets))
        if missing_targets:
            raise LlvmToolchainConfigError(
                "canonical LLVM target set is missing required targets: "
                f"missing={missing_targets} found={list(verification.targets)}"
            )
        expected_build_type = canonical_llvm_build_type(root)
        if resolved_build_type != expected_build_type:
            raise LlvmToolchainConfigError(
                "canonical LLVM build type must match the release manifest: "
                f"expected={expected_build_type} found={resolved_build_type}"
            )
    llvm_config_relative = verification.llvm_config.relative_to(verification.prefix)
    payload = {
        "schema": LLVM_ATTESTATION_SCHEMA,
        "architecture_contract_sha256": contract.digest,
        "version": verification.version,
        "prefix": str(attested_prefix),
        "projects": sorted(projects),
        "targets": list(verification.targets),
        "assets": list(verification.assets),
        "tool_versions": [asdict(fact) for fact in verification.tool_versions],
        "library_facts": [asdict(fact) for fact in verification.library_facts],
        "content_facts": [asdict(fact) for fact in content_facts],
        "content_digest": content_digest,
        "link_closure": list(verification.link_closure),
        "link_probe": list(verification.link_probe),
        "release": (asdict(release) if release is not None else None),
        "custody": custody,
        "release_manifest_sha256": load_llvm_releases(root).digest,
        "build_config": {
            "projects": sorted(projects),
            "targets": list(verification.targets),
            "build_type": resolved_build_type,
        },
        "llvm_config": str(attested_prefix / llvm_config_relative),
    }
    path = llvm_attestation_path(verification.prefix)
    tmp = path.with_name(f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        with tmp.open("w", encoding="utf-8") as handle:
            json.dump(payload, handle, indent=2, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp, path)
    finally:
        if tmp.exists():
            tmp.unlink()
    return path


def _llvm_config_version(executable: str) -> tuple[int, int, str]:
    try:
        result = subprocess.run(
            [executable, "--version"],
            check=True,
            capture_output=True,
            text=True,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise LlvmToolchainConfigError(
            f"could not query LLVM/MLIR toolchain version from {executable}: {exc}"
        ) from exc
    rendered = result.stdout.strip()
    match = re.match(r"^(\d+)\.(\d+)(?:\.|$)", rendered)
    if match is None:
        raise LlvmToolchainConfigError(
            f"llvm-config returned an invalid version {rendered!r}: {executable}"
        )
    return int(match.group(1)), int(match.group(2)), rendered


def _normalized_prefix(path: str | Path) -> Path:
    return Path(path).expanduser().resolve(strict=False)


def resolve_llvm_toolchain_prefix(
    root: Path,
    *,
    environ: dict[str, str] | None = None,
) -> Path | None:
    """Resolve the prefix reported by the canonical discovery authority."""

    discovery = discover_llvm_toolchain(root, environ=environ)
    return None if discovery is None else discovery.prefix


def verify_available_llvm_toolchain(
    root: Path,
    *,
    environ: dict[str, str] | None = None,
    content_policy: Literal["cached", "full"] = "cached",
) -> LlvmPrefixVerification | None:
    """Discover and verify the complete host LLVM/MLIR/LLD/Polly SDK."""

    discovery = discover_llvm_toolchain(root, environ=environ)
    if discovery is None:
        return None
    pin = required_llvm_backend_pin(root)
    assert pin is not None
    managed = managed_llvm_prefix(root, pin).resolve()
    return verify_llvm_toolchain_prefix(
        root,
        discovery.prefix,
        version=pin.default_release,
        expected_targets=required_llvm_targets_for_host(root),
        require_attestation=discovery.prefix.resolve() == managed,
        llvm_config_override=discovery.llvm_config,
        content_policy=content_policy,
    )


def verify_wasm_ci_toolchain(
    root: Path,
    wasi_sysroot: Path,
    *,
    environ: dict[str, str] | None = None,
) -> WasmCiToolchainVerification:
    """Verify the minimal manifest-owned linker/sysroot pair used by Rust truth.

    The complete LLVM/MLIR SDK verifier remains the authority for native and
    MLIR jobs. Rust's default workspace truth needs only the same release's
    WebAssembly linker plus the pinned WASI C runtime, so this profile proves
    that exact pair without installing the full development SDK.
    """

    discovery = discover_llvm_toolchain(root, environ=environ)
    if discovery is None:
        raise LlvmToolchainConfigError(
            "manifest-owned LLVM discovery could not resolve wasm-ld"
        )
    pin = required_llvm_backend_pin(root)
    assert pin is not None
    suffix = ".exe" if os.name == "nt" else ""
    wasm_ld = _required_tool(discovery.prefix, f"wasm-ld{suffix}")
    wasm_ld_fact = _tool_version_fact(
        discovery.prefix,
        "wasm-ld",
        wasm_ld,
        expected_version=pin.default_release,
        exact_version=True,
    )

    reject_poison_toolchain_path(wasi_sysroot, authority="WASI sysroot")
    resolved_sysroot = normalize_wasi_sysroot(wasi_sysroot)
    if resolved_sysroot is None:
        raise LlvmToolchainConfigError(
            f"WASI sysroot has no supported wasip1 headers: {wasi_sysroot}"
        )
    release = load_llvm_releases(root).wasi_sysroot
    version_file = resolved_sysroot / "VERSION"
    actual_llvm_version = wasi_sysroot_llvm_version(resolved_sysroot)
    if actual_llvm_version is None:
        raise LlvmToolchainConfigError(
            f"WASI sysroot has no LLVM producer identity at {version_file}"
        )
    if actual_llvm_version != release.llvm_version:
        raise LlvmToolchainConfigError(
            "WASI sysroot LLVM identity does not match the manifest authority: "
            f"expected {release.llvm_version}, found {actual_llvm_version!r} at "
            f"{version_file}"
        )
    try:
        version_text = version_file.read_text(encoding="utf-8")
    except OSError as exc:
        raise LlvmToolchainConfigError(
            f"WASI sysroot VERSION is unavailable: {version_file}: {exc}"
        ) from exc
    version_lines = version_text.splitlines()
    actual_version = version_lines[0].strip() if version_lines else ""
    if actual_version != release.version:
        raise LlvmToolchainConfigError(
            "WASI sysroot release does not match the manifest authority: "
            f"expected {release.version}, found {actual_version!r} at {version_file}"
        )
    required_assets = (
        version_file,
        resolved_sysroot / "include" / "wasm32-wasip1" / "errno.h",
        resolved_sysroot / "lib" / "wasm32-wasip1" / "libc.a",
    )
    missing = tuple(path for path in required_assets if not path.is_file())
    if missing:
        raise LlvmToolchainConfigError(
            "WASI sysroot is incomplete; missing "
            + ", ".join(str(path) for path in missing)
        )
    return WasmCiToolchainVerification(
        llvm_prefix=discovery.prefix,
        wasm_ld=wasm_ld,
        wasm_ld_fact=wasm_ld_fact,
        sysroot=resolved_sysroot,
        sysroot_version=actual_version,
        sysroot_llvm_version=actual_llvm_version,
        sysroot_assets=tuple(
            str(path.relative_to(resolved_sysroot)).replace("\\", "/")
            for path in required_assets
        ),
    )


def project_wasm_ci_environment(
    verification: WasmCiToolchainVerification,
    *,
    environ: dict[str, str] | None = None,
) -> dict[str, str]:
    """Project the verified linker/sysroot identity to every shared resolver."""

    result = dict(os.environ if environ is None else environ)
    sysroot = str(verification.sysroot)
    result["MOLT_WASI_SYSROOT"] = sysroot
    result["WASI_SYSROOT"] = sysroot
    result["MOLT_WASM_LD"] = str(verification.wasm_ld)
    bin_text = str(verification.llvm_prefix / "bin")
    path_parts = [part for part in result.get("PATH", "").split(os.pathsep) if part]
    normalized_bin = os.path.normcase(os.path.normpath(bin_text))
    if all(
        os.path.normcase(os.path.normpath(part)) != normalized_bin
        for part in path_parts
    ):
        result["PATH"] = os.pathsep.join([bin_text, *path_parts])
    return result


def mlir_toolchain_environment(
    root: Path,
    *,
    environ: dict[str, str] | None = None,
    require: bool = True,
) -> dict[str, str]:
    """Verify one SDK identity and project each binding's required path shape."""

    pin = required_llvm_backend_pin(root)
    if pin is None:
        if not require:
            return dict(os.environ if environ is None else environ)
        raise LlvmToolchainConfigError(
            f"could not resolve LLVM backend feature pin under {root}"
        )
    result = dict(os.environ if environ is None else environ)
    verification = verify_available_llvm_toolchain(root, environ=result)
    if verification is None:
        if require:
            raise LlvmToolchainConfigError(
                "LLVM/MLIR toolchain is unavailable; run "
                f"{sys.executable} -m tools.bootstrap_llvm"
            )
        return result
    return project_llvm_toolchain_environment(root, verification, environ=result)


def project_llvm_toolchain_environment(
    root: Path,
    verification: LlvmPrefixVerification,
    *,
    environ: dict[str, str] | None = None,
) -> dict[str, str]:
    """Project an already verified result without rescanning the SDK.

    llvm-sys names its executable-search root ``PREFIX`` and searches only its
    ``bin`` child. Split package layouts therefore receive
    ``llvm_config.parent.parent`` there, while MLIR/TableGen receive the actual
    SDK prefix reported by ``llvm-config --prefix``.
    """

    pin = required_llvm_backend_pin(root)
    if pin is None:
        raise LlvmToolchainConfigError(
            f"could not resolve LLVM backend feature pin under {root}"
        )
    if verification.version != pin.default_release:
        raise LlvmToolchainConfigError(
            "cannot project an LLVM verification from the wrong release family: "
            f"expected exactly {pin.default_release}, found {verification.version}"
        )
    result = dict(os.environ if environ is None else environ)
    prefix = verification.prefix
    llvm_config = verification.llvm_config

    prefix_text = str(prefix)
    llvm_sys_search_prefix = llvm_config.parent.parent
    result["MOLT_LLVM_PREFIX"] = prefix_text
    result[pin.env_var] = str(llvm_sys_search_prefix)
    result[mlir_sys_prefix_env_var(pin.major)] = prefix_text
    result[tablegen_prefix_env_var(pin.major)] = prefix_text
    result["LLVM_CONFIG_PATH"] = str(llvm_config)

    bin_text = str(prefix / "bin")
    path_parts = [part for part in result.get("PATH", "").split(os.pathsep) if part]
    normalized_bin = os.path.normcase(os.path.normpath(bin_text))
    if all(
        os.path.normcase(os.path.normpath(part)) != normalized_bin
        for part in path_parts
    ):
        result["PATH"] = os.pathsep.join([bin_text, *path_parts])
    return result


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Resolve Molt's manifest-owned LLVM backend toolchain pin."
    )
    parser.add_argument("--root", type=Path, default=_repo_root())
    parser.add_argument(
        "--format",
        choices=("major", "env", "json"),
        default="major",
        help="stdout format; CI uses the default major-only output.",
    )
    parser.add_argument(
        "--github-output",
        type=Path,
        default=None,
        help="Append major/minor/env_var metadata to a GitHub Actions output file.",
    )
    parser.add_argument(
        "--github-env",
        type=Path,
        default=None,
        help="Verify the installed SDK and append its canonical environment to GITHUB_ENV.",
    )
    parser.add_argument(
        "--verify",
        action="store_true",
        help="Discover and verify the complete LLVM/MLIR/LLD/Polly developer SDK.",
    )
    parser.add_argument(
        "--verify-wasm",
        action="store_true",
        help="Verify the manifest-owned wasm-ld and pinned WASI sysroot profile.",
    )
    parser.add_argument(
        "--wasi-sysroot",
        type=Path,
        default=None,
        help="Extracted WASI sysroot to verify for --verify-wasm.",
    )
    args = parser.parse_args(argv)

    pin = required_llvm_backend_pin(args.root)
    if pin is None:
        print(
            f"could not resolve LLVM backend feature pin under {args.root}",
            file=sys.stderr,
        )
        return 1

    if args.github_output is not None:
        release_manifest = load_llvm_releases(args.root)
        with args.github_output.open("a", encoding="utf-8") as fh:
            fh.write(f"major={pin.major}\n")
            fh.write(f"minor={pin.minor}\n")
            fh.write(f"env_var={pin.env_var}\n")
            fh.write(f"feature={pin.inkwell_feature}\n")
            fh.write(f"release={pin.default_release}\n")
            fh.write(
                f"apt_packages={' '.join(llvm_debian_dev_packages(args.root, pin.major))}\n"
            )
            fh.write(f"apt_installer_url={release_manifest.debian_installer.url}\n")
            fh.write(
                f"apt_installer_sha256={release_manifest.debian_installer.sha256}\n"
            )
            wasi = release_manifest.wasi_sysroot
            fh.write(f"wasi_sysroot_version={wasi.version}\n")
            fh.write(f"wasi_sysroot_llvm_version={wasi.llvm_version}\n")
            fh.write(f"wasi_sysroot_url={wasi.url}\n")
            fh.write(f"wasi_sysroot_size={wasi.size}\n")
            fh.write(f"wasi_sysroot_sha256={wasi.sha256}\n")
            fh.write(f"wasi_sysroot_archive_root={wasi.archive_root}\n")

    wasm_verification: WasmCiToolchainVerification | None = None
    if args.verify_wasm:
        if args.wasi_sysroot is None:
            parser.error("--verify-wasm requires --wasi-sysroot")
        try:
            wasm_verification = verify_wasm_ci_toolchain(
                args.root,
                args.wasi_sysroot,
            )
        except LlvmToolchainConfigError as exc:
            print(f"WASM toolchain verification failed: {exc}", file=sys.stderr)
            return 2
        if args.github_env is not None:
            projected = project_wasm_ci_environment(
                wasm_verification,
                environ=dict(os.environ),
            )
            keys = ("MOLT_WASI_SYSROOT", "WASI_SYSROOT", "MOLT_WASM_LD", "PATH")
            with args.github_env.open("a", encoding="utf-8") as fh:
                for key in keys:
                    fh.write(f"{key}={projected[key]}\n")

    verification: LlvmPrefixVerification | None = None
    if args.verify or (args.github_env is not None and not args.verify_wasm):
        try:
            verification = verify_available_llvm_toolchain(args.root)
        except LlvmToolchainConfigError as exc:
            print(f"LLVM/MLIR toolchain verification failed: {exc}", file=sys.stderr)
            return 2
        if verification is None:
            print(
                "LLVM/MLIR toolchain is unavailable; run "
                f"{llvm_bootstrap_command(pin, python=sys.executable)}",
                file=sys.stderr,
            )
            return 2
        if args.github_env is not None:
            projected = project_llvm_toolchain_environment(
                args.root, verification, environ=dict(os.environ)
            )
            keys = (
                "MOLT_LLVM_PREFIX",
                pin.env_var,
                mlir_sys_prefix_env_var(pin.major),
                tablegen_prefix_env_var(pin.major),
                "LLVM_CONFIG_PATH",
                "PATH",
            )
            with args.github_env.open("a", encoding="utf-8") as fh:
                for key in keys:
                    fh.write(f"{key}={projected[key]}\n")

    if wasm_verification is not None and args.format == "json":
        print(
            json.dumps(
                {
                    "llvm_prefix": str(wasm_verification.llvm_prefix),
                    "wasm_ld": str(wasm_verification.wasm_ld),
                    "wasm_ld_fact": asdict(wasm_verification.wasm_ld_fact),
                    "wasi_sysroot": str(wasm_verification.sysroot),
                    "wasi_sysroot_version": wasm_verification.sysroot_version,
                    "wasi_sysroot_llvm_version": (
                        wasm_verification.sysroot_llvm_version
                    ),
                    "wasi_sysroot_assets": list(wasm_verification.sysroot_assets),
                },
                sort_keys=True,
            )
        )
    elif verification is not None and args.format == "json":
        print(
            json.dumps(
                {
                    "prefix": str(verification.prefix),
                    "llvm_config": str(verification.llvm_config),
                    "version": verification.version,
                    "targets": list(verification.targets),
                    "tools": [asdict(fact) for fact in verification.tool_versions],
                    "library_count": len(verification.library_facts),
                    "content_digest": verification.content_digest,
                },
                sort_keys=True,
            )
        )
    elif args.format == "major":
        print(pin.major)
    elif args.format == "env":
        print(pin.env_var)
    else:
        print(json.dumps(asdict(pin) | {"env_var": pin.env_var}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
