from __future__ import annotations

import argparse
from functools import lru_cache
import json
import os
import re
import shutil
import subprocess
import sys
import tomllib
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any


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
    if major == 22 and minor == 1:
        return "22.1.8"
    return f"{major}.{minor}.0"


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
    return (
        root.resolve()
        / "target"
        / "toolchains"
        / f"llvm-{resolved_pin.default_release}"
    )


def llvm_config_executable(prefix: Path) -> Path:
    """Resolve ``llvm-config`` without assuming the host filename convention."""

    bin_dir = prefix / "bin"
    windows = bin_dir / "llvm-config.exe"
    return windows if windows.is_file() else bin_dir / "llvm-config"


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
                f"{sys.executable} tools/bootstrap_llvm.py"
            )
        return result
    llvm_config = llvm_config_executable(prefix)
    if not llvm_config.is_file():
        raise LlvmToolchainConfigError(
            f"LLVM/MLIR prefix does not contain llvm-config: {llvm_config}"
        )
    actual_major, actual_minor, actual_version = _llvm_config_version(str(llvm_config))
    if (actual_major, actual_minor) != (pin.major, pin.minor):
        raise LlvmToolchainConfigError(
            "LLVM/MLIR toolchain version does not match the manifest authority: "
            f"expected {pin.major}.{pin.minor}.x, found {actual_version} at {llvm_config}"
        )

    prefix_text = str(prefix)
    result["MOLT_LLVM_PREFIX"] = prefix_text
    result[pin.env_var] = prefix_text
    result[mlir_sys_prefix_env_var(pin.major)] = prefix_text
    result[tablegen_prefix_env_var(pin.major)] = prefix_text
    result["LLVM_CONFIG_PATH"] = str(llvm_config)

    bin_text = str(prefix / "bin")
    path_parts = [part for part in result.get("PATH", "").split(os.pathsep) if part]
    normalized_bin = os.path.normcase(os.path.normpath(bin_text))
    if all(os.path.normcase(os.path.normpath(part)) != normalized_bin for part in path_parts):
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
