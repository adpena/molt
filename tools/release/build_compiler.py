"""Build the portable production compiler, never a guest/runtime profile."""

from __future__ import annotations

import argparse
from collections.abc import Mapping
import os
from pathlib import Path
import tomllib

from molt.cargo_execution_policy import CARGO_WRAPPER_ENV_NAMES
from molt.compiler_distribution import (
    PRODUCTION_COMPILER_FEATURES,
    PRODUCTION_COMPILER_PROFILE,
)
from tools.command_execution import CommandExecutor
from molt.rust_toolchain import cargo_configuration_paths


def _production_control(name: str) -> bool:
    name = name.upper()
    return (
        name.startswith("CARGO_PROFILE_")
        or name in CARGO_WRAPPER_ENV_NAMES
        or name
        in {
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_BUILD_RUSTFLAGS",
            "CARGO_BUILD_TARGET",
            "CARGO_INCREMENTAL",
            "RUSTUP_TOOLCHAIN",
            "RUSTC",
            "CARGO_BUILD_RUSTC",
            "RUSTC_BOOTSTRAP",
        }
        or (name.startswith("CARGO_TARGET_") and name.endswith("_RUSTFLAGS"))
    )


def production_environment(root: Path, inherited: Mapping[str, str]) -> dict[str, str]:
    """Keep the manifest/toolchain policy independent of developer overrides.

    Encoded Rust flags outrank target/cfg flags and preserve source paths with
    spaces. Cargo profile tables in config outrank Cargo.toml, so reject them
    rather than maintain another copy of the manifest's optimization policy.
    """
    env = {
        key: value for key, value in inherited.items() if not _production_control(key)
    }
    for path in cargo_configuration_paths(root, env):
        with path.open("rb") as stream:
            config = tomllib.load(stream)
        build = config.get("build", {})
        configured_env = config.get("env", {})
        if (
            "include" in config
            or "profile" in config
            or not isinstance(build, dict)
            or any(key in build for key in ("target", "rustc"))
            or not isinstance(configured_env, dict)
            or any(_production_control(name) for name in configured_env)
        ):
            raise ValueError(
                f"production compiler policy is overridden by Cargo config: {path}"
            )
    with (root / "rust-toolchain.toml").open("rb") as stream:
        env["RUSTUP_TOOLCHAIN"] = tomllib.load(stream)["toolchain"]["channel"]
    # Explicit empty values also disable wrappers selected in Cargo config.
    env.update({name: "" for name in CARGO_WRAPPER_ENV_NAMES})
    env["CARGO_INCREMENTAL"] = "0"
    env["CARGO_ENCODED_RUSTFLAGS"] = f"--remap-path-prefix={root}=/molt"
    return env


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-dir", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    env = production_environment(root, os.environ)
    commands = CommandExecutor.for_file(__file__)
    commands.run(
        [
            "cargo",
            "build",
            "--locked",
            "--timings",
            "--profile",
            PRODUCTION_COMPILER_PROFILE,
            "-p",
            "molt-backend",
            "--bin",
            "molt-backend",
            "--no-default-features",
            "--features",
            ",".join(PRODUCTION_COMPILER_FEATURES),
            "--target-dir",
            str(args.target_dir),
        ],
        cwd=root,
        env=env,
        check=True,
    )
    # The public entry point is part of the same pinned source/toolchain build,
    # but has no backend features or runtime dependencies.
    commands.run(
        [
            "cargo",
            "build",
            "--locked",
            "--profile",
            PRODUCTION_COMPILER_PROFILE,
            "-p",
            "molt-launcher",
            "--bin",
            "molt",
            "--target-dir",
            str(args.target_dir),
        ],
        cwd=root,
        env=env,
        check=True,
    )


if __name__ == "__main__":
    main()
