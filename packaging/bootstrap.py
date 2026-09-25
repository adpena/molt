#!/usr/bin/env python3
"""Launch the signed bundle through uv's locked environment authority.

Molt owns source/compiler identity; uv owns Python environment installation,
locking and cache reuse. No shared hand-maintained venv or activation is needed.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


def _resolve_root() -> Path:
    return Path(__file__).resolve().parents[2]


def _release_inputs(root: Path) -> tuple[Path, str, str]:
    wheels = sorted((root / "share" / "molt" / "wheels").glob("molt-*.whl"))
    if len(wheels) != 1:
        raise SystemExit("molt: bundle must contain exactly one Molt wheel")
    manifest = json.loads(
        (root / "source" / "release-compiler-source.json").read_text(encoding="utf-8")
    )
    record = manifest["wheel"]
    wheel = wheels[0]
    with wheel.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    if (wheel.name, wheel.stat().st_size, digest) != (
        record["filename"],
        record["size"],
        record["sha256"],
    ):
        raise SystemExit("molt: bundled wheel differs from the release manifest")
    wheel_digest = digest
    inputs = {entry["path"]: entry for entry in manifest["files"]}
    identity = hashlib.sha256(bytes.fromhex(digest))
    for name in ("pyproject.toml", "uv.lock"):
        path = root / "source" / name
        record = inputs[name]
        with path.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        if (path.stat().st_size, digest) != (record["size"], record["sha256"]):
            raise SystemExit(f"molt: bundled {name} differs from the release manifest")
        identity.update(bytes.fromhex(digest))
    # Separate immutable release/interpreter generations; uv owns environment
    # creation, installation locks and warm synchronization within a generation.
    identity.update(
        json.dumps([sys.executable, sys.version, sys.implementation.cache_tag]).encode()
    )
    return wheel, wheel_digest, identity.hexdigest()


def _prepare_environment(
    uv: str,
    root: Path,
    wheel: Path,
    wheel_digest: str,
    environment: Path,
    env: dict[str, str],
) -> Path:
    env["UV_PROJECT_ENVIRONMENT"] = str(environment)
    subprocess.run(
        [
            uv,
            "sync",
            "--no-config",
            "--project",
            str(root / "source"),
            "--frozen",
            "--no-dev",
            "--no-default-groups",
            "--no-install-project",
            "--inexact",
            "--no-build",
            "--no-python-downloads",
            "--python",
            sys.executable,
        ],
        env=env,
        check=True,
    )
    python = environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    subprocess.run(
        [
            uv,
            "pip",
            "install",
            "--no-config",
            "--python",
            str(python),
            "--no-deps",
            "--no-index",
            "--no-build",
            "--require-hashes",
            "-r",
            "-",
        ],
        input=f"{wheel.as_uri()} --hash=sha256:{wheel_digest}\n",
        text=True,
        env=env,
        check=True,
    )
    return python


def main() -> None:
    if sys.implementation.name != "cpython" or sys.version_info < (3, 12):
        raise SystemExit("molt: CPython 3.12+ is required")
    uv = shutil.which("uv")
    if uv is None:
        raise SystemExit(
            "molt: uv is required; install it from https://docs.astral.sh/uv/"
        )
    root = _resolve_root()
    try:
        wheel, wheel_digest, identity = _release_inputs(root)
    except (OSError, ValueError, KeyError, TypeError) as exc:
        raise SystemExit(f"molt: invalid release bundle: {exc}") from exc
    env = os.environ.copy()
    # User project/resolver/install policy cannot alter the shipped dependency
    # closure. Keep only uv's operational cache/offline controls.
    for name in tuple(env):
        if name in {"PYTHONPATH", "PYTHONHOME", "VIRTUAL_ENV"} or (
            name.startswith("UV_") and name not in {"UV_CACHE_DIR", "UV_OFFLINE"}
        ):
            env.pop(name)
    env["MOLT_SOURCE_ROOT"] = str(root / "source")
    env["MOLT_BUNDLE_ROOT"] = str(root)
    env.setdefault("MOLT_PROJECT_ROOT", os.getcwd())
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    home = Path(env.get("MOLT_HOME", str(Path.home() / ".molt"))).expanduser().resolve()
    # Source inputs may be read-only. Mutable outputs have an independent home.
    env.setdefault("CARGO_TARGET_DIR", str(home / "target"))
    env.setdefault("MOLT_BUILD_STATE_DIR", str(home / "build-state"))
    try:
        python = _prepare_environment(
            uv, root, wheel, wheel_digest, home / "environments" / identity, env
        )
    except subprocess.CalledProcessError as exc:
        raise SystemExit(exc.returncode) from exc
    args = [str(python), "-I", "-B", "-m", "molt.cli", *sys.argv[1:]]
    if os.name == "nt":
        # Windows CRT exec overlays detach and report success before the child
        # exits. Keep custody and propagate the actual CLI status instead.
        raise SystemExit(subprocess.call(args, env=env))
    os.execve(python, args, env)


if __name__ == "__main__":
    main()
