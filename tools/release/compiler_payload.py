"""Assemble the committed source closure, production compiler and runtime cells."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

from molt.compiler_distribution import (
    MANIFEST_NAME,
    MANIFEST_SCHEMA,
    MAX_SOURCE_BYTES,
    MAX_SOURCE_FILES,
    PRODUCTION_COMPILER_PROFILE,
    PRODUCTION_COMPILER_FEATURES,
    validate_compiler_record,
    validate_launcher_record,
    validate_wasm_facts_record,
)
from molt.host_tool_products import HostToolProduct
from molt.exact_json import write_exact
from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder
from molt.toolchain_identity import (
    resolve_executable,
    stable_regular_file_content_identity,
)
from molt.cli.native_binary import validate_native_binary_architecture
from molt.release_matrix import RUST_TARGET_BY_COORDINATE
from .git_source_snapshot import (
    GitSourceSnapshot,
    capture_git_source_snapshot,
    materialize_git_source_snapshot,
    immutable_git_environment,
    read_git_commit_epoch,
)

# Harvested from the preserved release-source lane; current config consumers
# additionally require differential suite paths and MLIR's vendored dependency.
SOURCE_PATHS = (
    ".cargo",
    "config",
    "docs/spec",
    "include",
    "packaging/INSTALL.md",
    "packaging/bootstrap.py",
    "runtime",
    "src",
    "tools",
    "tests/differential",
    "uv.lock",
    "vendor",
    "wasm",
    "Cargo.lock",
    "Cargo.toml",
    "LICENSE",
    "pyproject.toml",
    "rust-toolchain.toml",
)
# Source evidence supports repository capture/report/generation, not compilation.
# Keep this exact omission shared by native builds, runtime cells, and bundles.
DEVELOPMENT_ONLY_SOURCE_PATHS = frozenset({"config/cpython_stdlib_snapshot.json"})
REQUIRED_MARKERS = frozenset(
    {
        "Cargo.toml",
        "Cargo.lock",
        "pyproject.toml",
        "uv.lock",
        "rust-toolchain.toml",
        "runtime/molt-backend/Cargo.toml",
        "runtime/molt-wasm-facts/Cargo.toml",
        "runtime/molt-runtime/Cargo.toml",
        "src/molt/cli/__main__.py",
        "src/molt/backend_environment.json",
        "src/molt/backend_daemon_protocol_version.txt",
        "src/molt/stdlib/builtins.py",
        "packaging/bootstrap.py",
        "packaging/INSTALL.md",
        "LICENSE",
        "vendor/llvm/LICENSE.TXT",
    }
)


def source_environment() -> ProcessEnvironmentBuilder:
    inherited = ProcessEnvironment(os.environ)
    allowed = ProcessEnvironment(
        dict.fromkeys(
            {
                "COMSPEC",
                "HOME",
                "HOMEDRIVE",
                "HOMEPATH",
                "PATH",
                "PATHEXT",
                "SYSTEMDRIVE",
                "SYSTEMROOT",
                "TEMP",
                "TMP",
                "USERPROFILE",
                "WINDIR",
            },
            "",
        ),
        rule=inherited.rule,
    )
    env = ProcessEnvironment(
        {key: value for key, value in inherited.items() if key in allowed},
        rule=inherited.rule,
    ).updated(
        {
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_OPTIONAL_LOCKS": "0",
            "LC_ALL": "C",
        }
    )
    return immutable_git_environment(env)


def reproducible_build_environment(epoch: int) -> dict[str, str]:
    """Release-only Cargo and C metadata policy, shared by native and runtime builds."""
    if type(epoch) is not int or epoch <= 0:
        raise ValueError("release source epoch must be a positive integer")
    return {
        "CARGO_INCREMENTAL": "0",
        "SOURCE_DATE_EPOCH": str(epoch),
        "LC_ALL": "C",
        "TZ": "UTC",
    }


def source_commit_epoch(repo_root: Path, source_sha: str) -> int:
    env = source_environment()
    return read_git_commit_epoch(
        repo_root,
        source_sha,
        git=resolve_executable("git", environment=env, label="release source Git"),
        environment=env,
    )


def source_snapshot(repo_root: Path, source_sha: str) -> GitSourceSnapshot:
    env = source_environment()
    return capture_git_source_snapshot(
        repo_root,
        source_sha,
        git=resolve_executable("git", environment=env, label="release source Git"),
        environment=env,
        pathspecs=SOURCE_PATHS,
        excluded_paths=DEVELOPMENT_ONLY_SOURCE_PATHS,
        required_markers=REQUIRED_MARKERS,
        max_files=MAX_SOURCE_FILES,
        max_bytes=MAX_SOURCE_BYTES,
    )


def compiler_record(binary: Path, *, platform: str, arch: str) -> dict[str, Any]:
    validate_native_binary_architecture(
        binary, RUST_TARGET_BY_COORDINATE[(platform, arch)]
    )
    identity = stable_regular_file_content_identity(
        binary, label="production compiler input"
    )
    name = HostToolProduct.BACKEND.value + (".exe" if platform == "windows" else "")
    return validate_compiler_record(
        {
            "path": f"bin/{name}",
            "sha256": identity["sha256"],
            "size": identity["size"],
            "profile": PRODUCTION_COMPILER_PROFILE,
            "features": list(PRODUCTION_COMPILER_FEATURES),
            "platform": platform,
            "arch": arch,
        }
    )


def launcher_record(binary: Path, *, platform: str, arch: str) -> dict[str, Any]:
    validate_native_binary_architecture(
        binary, RUST_TARGET_BY_COORDINATE[(platform, arch)]
    )
    identity = stable_regular_file_content_identity(
        binary, label="production launcher input"
    )
    return validate_launcher_record(
        {
            "path": "bin/molt.exe" if platform == "windows" else "bin/molt",
            "sha256": identity["sha256"],
            "size": identity["size"],
            "platform": platform,
            "arch": arch,
        }
    )


def wasm_facts_record(binary: Path, *, platform: str, arch: str) -> dict[str, Any]:
    validate_native_binary_architecture(
        binary, RUST_TARGET_BY_COORDINATE[(platform, arch)]
    )
    identity = stable_regular_file_content_identity(
        binary, label="production WASM facts input"
    )
    name = HostToolProduct.WASM_FACTS.value + (".exe" if platform == "windows" else "")
    return validate_wasm_facts_record(
        {
            "path": f"bin/{name}",
            "sha256": identity["sha256"],
            "size": identity["size"],
            "profile": PRODUCTION_COMPILER_PROFILE,
            "platform": platform,
            "arch": arch,
        }
    )


def materialize_sources(
    root: Path,
    *,
    repo_root: Path,
    snapshot: GitSourceSnapshot,
    compiler: dict[str, Any],
    launcher: dict[str, Any],
    wasm_facts: dict[str, Any],
    runtime: dict[str, Any],
) -> Path:
    env = source_environment()
    source = materialize_git_source_snapshot(
        snapshot,
        root / "source",
        repo_root=repo_root,
        git=resolve_executable("git", environment=env, label="release source Git"),
        environment=env,
    )
    write_exact(
        source / MANIFEST_NAME,
        {
            "schema": MANIFEST_SCHEMA,
            "git": {
                "object_format": snapshot.object_format,
                "commit": snapshot.source_sha,
                "tree": snapshot.tree_sha,
            },
            "files": [entry.as_record() for entry in snapshot.files],
            "compiler": compiler,
            "launcher": launcher,
            "wasm_facts": wasm_facts,
            "runtime": runtime,
        },
    )
    return source
