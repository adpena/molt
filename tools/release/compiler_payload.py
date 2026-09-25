"""Assemble the committed source closure and production compiler as one payload."""

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
)
from molt.exact_json import write_exact
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
REQUIRED_MARKERS = frozenset(
    {
        "Cargo.toml",
        "Cargo.lock",
        "pyproject.toml",
        "uv.lock",
        "runtime/molt-backend/Cargo.toml",
        "runtime/molt-runtime/Cargo.toml",
        "src/molt/cli/__main__.py",
        "src/molt/stdlib/builtins.py",
        "packaging/bootstrap.py",
        "packaging/INSTALL.md",
        "LICENSE",
    }
)


def source_environment() -> dict[str, str]:
    allowed = {
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
    }
    env = {key: value for key, value in os.environ.items() if key.upper() in allowed}
    env.update(
        GIT_CONFIG_GLOBAL=os.devnull,
        GIT_CONFIG_NOSYSTEM="1",
        GIT_OPTIONAL_LOCKS="0",
        LC_ALL="C",
    )
    return env


def source_snapshot(repo_root: Path, source_sha: str) -> GitSourceSnapshot:
    env = source_environment()
    return capture_git_source_snapshot(
        repo_root,
        source_sha,
        git=resolve_executable("git", environment=env, label="release source Git"),
        environment=env,
        pathspecs=SOURCE_PATHS,
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
    name = "molt-backend.exe" if platform == "windows" else "molt-backend"
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


def materialize_sources(
    root: Path,
    *,
    repo_root: Path,
    snapshot: GitSourceSnapshot,
    compiler: dict[str, Any],
    wheel: Path,
) -> Path:
    env = source_environment()
    source = materialize_git_source_snapshot(
        snapshot,
        root / "source",
        repo_root=repo_root,
        git=resolve_executable("git", environment=env, label="release source Git"),
        environment=env,
    )
    wheel_identity = stable_regular_file_content_identity(wheel, label="release wheel")
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
            "wheel": {
                "filename": wheel.name,
                "sha256": wheel_identity["sha256"],
                "size": wheel_identity["size"],
            },
        },
    )
    return source
