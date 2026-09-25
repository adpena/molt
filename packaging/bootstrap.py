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
import signal
import shlex
import subprocess
import sys


def _release_inputs(root: Path) -> tuple[Path, str]:
    manifest = json.loads(
        (root / "source" / "release-compiler-source.json").read_text(encoding="utf-8")
    )
    launcher = manifest["launcher"]
    launcher_name = "molt.exe" if os.name == "nt" else "molt"
    if launcher["path"] != f"bin/{launcher_name}":
        raise SystemExit("molt: release launcher path is invalid")
    executable = root / launcher["path"]
    with executable.open("rb") as stream:
        launcher_digest = hashlib.file_digest(stream, "sha256").hexdigest()
    if (executable.stat().st_size, launcher_digest) != (
        launcher["size"],
        launcher["sha256"],
    ):
        raise SystemExit("molt: launcher differs from the release manifest")
    inputs = {entry["path"]: entry for entry in manifest["files"]}
    identity = hashlib.sha256(json.dumps(manifest["git"], sort_keys=True).encode())
    home = None
    for name in ("pyproject.toml", "uv.lock", "src/molt/cli/default_paths.py"):
        path = root / "source" / name
        record = inputs[name]
        content = path.read_bytes()
        digest = hashlib.sha256(content).hexdigest()
        if (len(content), digest) != (record["size"], record["sha256"]):
            raise SystemExit(f"molt: bundled {name} differs from the release manifest")
        identity.update(bytes.fromhex(digest))
        if name == "src/molt/cli/default_paths.py":
            # Reuse the CLI's stdlib-only path authority without importing the
            # CLI or running an unverified on-disk copy of this module.
            namespace = {"__name__": "molt_bootstrap_default_paths"}
            exec(compile(content, str(path), "exec"), namespace)
            home = namespace["_default_molt_home"]().resolve()
    assert home is not None
    if any(
        parent.exists() and parent.samefile(root) for parent in (home, *home.parents)
    ):
        raise SystemExit("molt: MOLT_HOME must be outside the immutable bundle")
    # Separate immutable release/interpreter generations; uv owns environment
    # creation, installation locks and warm synchronization within a generation.
    identity.update(
        json.dumps([sys.executable, sys.version, sys.implementation.cache_tag]).encode()
    )
    return home, identity.hexdigest()


def _prepare_environment(
    uv: str,
    root: Path,
    environment: Path,
    env: dict[str, str],
    *,
    install: bool,
) -> Path:
    env["UV_PROJECT_ENVIRONMENT"] = str(environment)
    python = environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    command = [
        uv,
        "sync",
        "--no-config",
        "--project",
        str(root / "source"),
        "--frozen",
        "--no-dev",
        "--no-default-groups",
        "--no-install-project",
        "--no-build",
        "--no-python-downloads",
        "--python",
        sys.executable,
    ]
    description = (
        f"Molt CLI source: {root / 'source'}\n"
        f"Python: {sys.executable}\n"
        f"Dependency manager: {uv}\n"
        f"Dependency environment: {environment}\n"
        f"Dependency authority: {root / 'source' / 'uv.lock'}\n"
        "Setup downloads and installs the locked CLI dependencies into this private "
        "environment, removing unrequested packages only there. It does not install "
        "Python or toolchains, change PATH, or "
        "modify other installations.\n"
    )
    if install:
        print(description, file=sys.stderr)
        subprocess.run(command, env=env, check=True)
    else:
        # uv owns synchronization checks as well as explicit installation. A
        # normal invocation must not download, install, remove or repair tools.
        ready = (
            python.is_file()
            and subprocess.run(
                [*command, "--check", "--offline", "--quiet"], env=env, check=False
            ).returncode
            == 0
        )
        if not ready:
            name = "molt.exe" if os.name == "nt" else "molt"
            launcher = str(root / "bin" / name)
            invocation = (
                "& '" + launcher.replace("'", "''") + "'"
                if os.name == "nt"
                else shlex.quote(launcher)
            )
            raise SystemExit(
                description + "No dependencies were changed. To authorize setup, run:\n"
                f"  {invocation} setup --install-cli-dependencies"
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
    if len(sys.argv) < 2:
        raise SystemExit("molt: native launcher must supply the bundle root")
    try:
        root = Path(sys.argv.pop(1)).resolve(strict=True)
        home, identity = _release_inputs(root)
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
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    env["RUSTUP_AUTO_INSTALL"] = "0"
    env["MOLT_HOME"] = str(home)
    # Source inputs may be read-only. Mutable outputs have an independent home.
    env.setdefault("CARGO_TARGET_DIR", str(home / "target"))
    env.setdefault("MOLT_BUILD_STATE_DIR", str(home / "build-state"))
    if os.name == "nt":
        # Console events also reach the child. Only the waiting parent ignores
        # them; a Python handler is not inherited by the child's interpreter.
        for event in (signal.SIGINT, signal.SIGBREAK):
            signal.signal(event, lambda _signum, _frame: None)
    try:
        install = sys.argv[1:] == ["setup", "--install-cli-dependencies"]
        python = _prepare_environment(
            uv, root, home / "environments" / identity, env, install=install
        )
    except subprocess.CalledProcessError as exc:
        raise SystemExit(exc.returncode) from exc
    except OSError as exc:
        raise SystemExit(
            f"molt: cannot inspect or prepare CLI dependencies: {exc}"
        ) from exc
    if install:
        print("Molt CLI dependencies are ready. Run molt doctor to inspect toolchains.")
        return
    # Re-entering Python children (REPL, extension tooling) inherit the same
    # source authority, not a missing or ambient site-packages Molt copy.
    env["PYTHONPATH"] = str(root / "source" / "src")
    # The bundled source is the only Molt import authority. The uv environment
    # contains dependencies only, never a second installed copy of Molt.
    args = [
        str(python),
        "-I",
        "-B",
        "-c",
        "import runpy,sys; sys.path.insert(0,sys.argv.pop(1)); "
        "runpy.run_module('molt.cli',run_name='__main__',alter_sys=True)",
        str(root / "source" / "src"),
        *sys.argv[1:],
    ]
    if os.name == "nt":
        # Windows CRT exec overlays detach and report success before the child
        # exits. Keep custody and propagate the actual CLI status instead.
        code = subprocess.call(args, env=env)
        raise SystemExit(code - (1 << 32) if code > 0x7FFFFFFF else code)
    os.execve(python, args, env)


if __name__ == "__main__":
    main()
