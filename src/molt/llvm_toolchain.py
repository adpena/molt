from __future__ import annotations

import argparse
import json
import re
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
