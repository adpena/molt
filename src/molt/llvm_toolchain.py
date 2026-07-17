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
class LlvmRelease:
    version: str
    url: str
    size: int
    source_sha256: str
    provenance_url: str
    record_sha256: str


@dataclass(frozen=True)
class LlvmReleaseManifest:
    schema_version: int
    default_release: str
    canonical_build_type: str
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
    if payload.get("schema_version") != 1:
        raise LlvmToolchainConfigError(
            f"unsupported LLVM release manifest schema at {path}"
        )
    default = payload.get("default_release")
    canonical_build_type = payload.get("canonical_build_type")
    rows = payload.get("releases")
    if (
        not isinstance(default, str)
        or canonical_build_type not in {"Release", "RelWithDebInfo", "Debug"}
        or not isinstance(rows, dict)
    ):
        raise LlvmToolchainConfigError(f"incomplete LLVM release manifest: {path}")
    releases: list[LlvmRelease] = []
    for version, row in rows.items():
        if not isinstance(version, str) or not isinstance(row, dict):
            raise LlvmToolchainConfigError(f"invalid LLVM release row in {path}")
        required = ("url", "size", "source_sha256", "provenance_url")
        if any(name not in row for name in required):
            raise LlvmToolchainConfigError(
                f"LLVM release {version} is incomplete in {path}"
            )
        url = row["url"]
        size = row["size"]
        source_sha256 = row["source_sha256"]
        provenance_url = row["provenance_url"]
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
                record_sha256=record_sha256,
            )
        )
    if default not in {release.version for release in releases}:
        raise LlvmToolchainConfigError(
            f"default LLVM release {default!r} is not declared in {path}"
        )
    return LlvmReleaseManifest(
        schema_version=1,
        default_release=default,
        canonical_build_type=str(canonical_build_type),
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


def llvm_config_executable(prefix: Path) -> Path:
    """Resolve ``llvm-config`` without assuming the host filename convention."""

    bin_dir = prefix / "bin"
    windows = bin_dir / "llvm-config.exe"
    return windows if windows.is_file() else bin_dir / "llvm-config"


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
    try:
        linker_identity = str(
            host_linker.resolve().relative_to(prefix.resolve())
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


@lru_cache(maxsize=1)
def _windows_change_time_api() -> tuple[Any, Any, Any] | None:
    if os.name != "nt":
        return None
    try:
        import ctypes
        from ctypes import wintypes

        class FileBasicInfo(ctypes.Structure):
            _fields_ = [
                ("CreationTime", ctypes.c_longlong),
                ("LastAccessTime", ctypes.c_longlong),
                ("LastWriteTime", ctypes.c_longlong),
                ("ChangeTime", ctypes.c_longlong),
                ("FileAttributes", wintypes.DWORD),
            ]

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CreateFileW.restype = wintypes.HANDLE
        kernel32.CreateFileW.argtypes = [
            wintypes.LPCWSTR,
            wintypes.DWORD,
            wintypes.DWORD,
            ctypes.c_void_p,
            wintypes.DWORD,
            wintypes.DWORD,
            wintypes.HANDLE,
        ]
        kernel32.GetFileInformationByHandleEx.restype = wintypes.BOOL
        kernel32.GetFileInformationByHandleEx.argtypes = [
            wintypes.HANDLE,
            ctypes.c_int,
            ctypes.c_void_p,
            wintypes.DWORD,
        ]
        kernel32.CloseHandle.restype = wintypes.BOOL
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        return ctypes, kernel32, FileBasicInfo
    except (AttributeError, OSError, ValueError):
        return None


def _windows_change_time_ns(path: Path) -> int | None:
    api = _windows_change_time_api()
    if api is None:
        return None
    ctypes, kernel32, file_basic_info = api
    try:
        handle = kernel32.CreateFileW(
            str(path),
            0x0080,
            0x00000001 | 0x00000002 | 0x00000004,
            None,
            3,
            0x02000000,
            None,
        )
        invalid_handle = ctypes.c_void_p(-1).value
        if handle in (None, invalid_handle):
            return None
        try:
            info = file_basic_info()
            if not kernel32.GetFileInformationByHandleEx(
                handle, 0, ctypes.byref(info), ctypes.sizeof(info)
            ):
                return None
            return int(info.ChangeTime) * 100
        finally:
            kernel32.CloseHandle(handle)
    except (AttributeError, OSError, ValueError):
        return None


def _content_change_time_ns(path: Path, stat: os.stat_result) -> int | None:
    if os.name == "nt":
        # Windows st_ctime is creation time and is never a substitute for the
        # NTFS ChangeTime query.  None deliberately forces content hashing.
        return _windows_change_time_ns(path)
    # POSIX ctime is inode-change time, so it is a valid metadata fast path.
    return stat.st_ctime_ns


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
    return LlvmToolVersionFact(
        role=role,
        path=str(path.relative_to(prefix)).replace("\\", "/"),
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
    expected_major_minor = ".".join(expected_version.split(".")[:2])
    release = llvm_release(expected_version, root)
    managed = managed_llvm_paths(root, pin, version=expected_version).prefix.resolve()
    canonical_contract = resolved == managed or require_attestation
    version_matches = (
        actual_version == expected_version
        if canonical_contract
        else actual_version == expected_major_minor
        or actual_version.startswith(expected_major_minor + ".")
    )
    if canonical_contract and release is None:
        raise LlvmToolchainConfigError(
            f"canonical LLVM/MLIR prefix requires a pinned release: {expected_version}"
        )
    if not version_matches:
        raise LlvmToolchainConfigError(
            "LLVM/MLIR toolchain version does not match the manifest authority: "
            f"expected {'exactly ' + expected_version if canonical_contract else expected_major_minor + '.x'}, "
            f"found {actual_version} at {llvm_config}"
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
    if canonical_contract and set(built_targets) != required_targets:
        raise LlvmToolchainConfigError(
            "canonical LLVM/MLIR prefix target set must match exactly: "
            f"expected={sorted(required_targets)} found={list(built_targets)}"
        )

    suffix = ".exe" if os.name == "nt" else ""
    system = platform.system()
    host_linker = (
        (f"lld-link{suffix}",)
        if system == "Windows"
        else (f"ld64.lld{suffix}",)
        if system == "Darwin"
        else (f"ld.lld{suffix}",)
    )
    tools = (
        ("llvm-config", llvm_config),
        ("clang", _required_tool(resolved, f"clang{suffix}")),
        ("clang++", _required_tool(resolved, f"clang++{suffix}")),
        (
            "lld",
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
    llvm_libraries = tuple(
        sorted(path for path in lib_dir.glob("*LLVM*") if path.is_file())
    )
    mlir_libraries = tuple(
        sorted(path for path in lib_dir.glob("*MLIR*") if path.is_file())
    )
    if not llvm_libraries:
        raise LlvmToolchainConfigError(f"LLVM libraries are missing from {lib_dir}")
    if not mlir_libraries:
        raise LlvmToolchainConfigError(f"MLIR libraries are missing from {lib_dir}")
    required_libraries = tuple(sorted(set(llvm_libraries).union(mlir_libraries)))
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
            expected_version=(
                expected_version if canonical_contract else expected_major_minor
            ),
            exact_version=canonical_contract,
        )

    with ThreadPoolExecutor(max_workers=len(tools)) as executor:
        tool_versions = tuple(executor.map(inspect_tool, tools))

    assets = tuple(
        sorted(
            str(path.relative_to(resolved)).replace("\\", "/")
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
        if set(verification.targets) != expected_targets:
            raise LlvmToolchainConfigError(
                "canonical LLVM target set must match exactly: "
                f"expected={sorted(expected_targets)} "
                f"found={list(verification.targets)}"
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


@lru_cache(maxsize=8)
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
    """Resolve one LLVM/MLIR prefix and reject split dependency custody.

    Explicit Molt/llvm-sys/mlir-sys/tblgen prefixes are one authority class.
    If more than one is set they must name the same installation. Otherwise the
    managed, manifest-versioned prefix wins, followed by ``llvm-config`` on
    ``PATH`` for system package-manager installations.
    """

    pin = required_llvm_backend_pin(root)
    if pin is None:
        return None
    env = os.environ if environ is None else environ
    authority_names = (
        "MOLT_LLVM_PREFIX",
        pin.env_var,
        mlir_sys_prefix_env_var(pin.major),
        tablegen_prefix_env_var(pin.major),
    )
    for name in authority_names:
        if value := env.get(name, "").strip():
            reject_poison_toolchain_path(value, authority=name)
    if llvm_config_path := env.get("LLVM_CONFIG_PATH", "").strip():
        reject_poison_toolchain_path(llvm_config_path, authority="LLVM_CONFIG_PATH")
    if target_root := env.get("MOLT_TARGET_ROOT", "").strip():
        reject_poison_toolchain_path(target_root, authority="MOLT_TARGET_ROOT")
    explicit = {
        _normalized_prefix(value)
        for name in authority_names
        if (value := env.get(name, "").strip())
    }
    if len(explicit) > 1:
        rendered = ", ".join(str(path) for path in sorted(explicit, key=str))
        raise LlvmToolchainConfigError(
            "LLVM/MLIR prefix authorities disagree; configure one toolchain family: "
            f"{rendered}"
        )
    if explicit:
        return next(iter(explicit))

    managed = managed_llvm_prefix(root, pin)
    if llvm_config_executable(managed).is_file():
        return managed

    discovered = shutil.which("llvm-config", path=env.get("PATH")) or shutil.which(
        "llvm-config.exe", path=env.get("PATH")
    )
    if discovered is None:
        return None
    reject_poison_toolchain_path(discovered, authority="PATH llvm-config")
    return Path(discovered).resolve().parent.parent


def mlir_toolchain_environment(
    root: Path,
    *,
    environ: dict[str, str] | None = None,
    require: bool = True,
) -> dict[str, str]:
    """Project one resolved LLVM prefix into every Rust binding authority."""

    pin = required_llvm_backend_pin(root)
    if pin is None:
        if not require:
            return dict(os.environ if environ is None else environ)
        raise LlvmToolchainConfigError(
            f"could not resolve LLVM backend feature pin under {root}"
        )
    result = dict(os.environ if environ is None else environ)
    prefix = resolve_llvm_toolchain_prefix(root, environ=result)
    if prefix is None:
        if require:
            raise LlvmToolchainConfigError(
                "LLVM/MLIR toolchain is unavailable; run "
                f"{sys.executable} -m tools.bootstrap_llvm"
            )
        return result
    managed = managed_llvm_prefix(root, pin).resolve()
    verification = verify_llvm_toolchain_prefix(
        root,
        prefix,
        version=pin.default_release,
        expected_targets=required_llvm_targets_for_host(root),
        require_attestation=prefix.resolve() == managed,
    )
    return project_llvm_toolchain_environment(root, verification, environ=result)


def project_llvm_toolchain_environment(
    root: Path,
    verification: LlvmPrefixVerification,
    *,
    environ: dict[str, str] | None = None,
) -> dict[str, str]:
    """Project an already verified result without rescanning the SDK."""

    pin = required_llvm_backend_pin(root)
    if pin is None:
        raise LlvmToolchainConfigError(
            f"could not resolve LLVM backend feature pin under {root}"
        )
    expected_major_minor = f"{pin.major}.{pin.minor}"
    if not (
        verification.version == expected_major_minor
        or verification.version.startswith(expected_major_minor + ".")
    ):
        raise LlvmToolchainConfigError(
            "cannot project an LLVM verification from the wrong release family: "
            f"expected {expected_major_minor}.x, found {verification.version}"
        )
    result = dict(os.environ if environ is None else environ)
    prefix = verification.prefix
    llvm_config = verification.llvm_config

    prefix_text = str(prefix)
    result["MOLT_LLVM_PREFIX"] = prefix_text
    result[pin.env_var] = prefix_text
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
    args = parser.parse_args(argv)

    pin = required_llvm_backend_pin(args.root)
    if pin is None:
        print(
            f"could not resolve LLVM backend feature pin under {args.root}",
            file=sys.stderr,
        )
        return 1

    if args.github_output is not None:
        with args.github_output.open("a", encoding="utf-8") as fh:
            fh.write(f"major={pin.major}\n")
            fh.write(f"minor={pin.minor}\n")
            fh.write(f"env_var={pin.env_var}\n")
            fh.write(f"feature={pin.inkwell_feature}\n")

    if args.format == "major":
        print(pin.major)
    elif args.format == "env":
        print(pin.env_var)
    else:
        print(json.dumps(asdict(pin) | {"env_var": pin.env_var}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
