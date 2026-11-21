"""A fake ``uv`` and a minimal checkout for shared-environment tests.

The fake records each call and builds a standard environment (``pyvenv.cfg``
and ``bin/python`` linked to the test's base interpreter), so the identity
probe and any child started from it run for real, with ``sys.prefix`` at the
environment.
"""

from __future__ import annotations

import os
import platform
import sys
from pathlib import Path

import pytest

PYPROJECT = """\
[project]
name = "subject"
version = "0"
description = "first"
requires-python = ">=3.12"
dependencies = []

[dependency-groups]
dev = ["pytest"]

[tool.ruff]
line-length = 88

[tool.molt.dx]
dependency_groups = ["dev"]
"""

FAKE_UV = """\
#!{python}
import json, os, sys
from pathlib import Path
with open({log!r}, "a", encoding="utf-8") as log:
    log.write(json.dumps(sys.argv[1:]) + "\\n")
if os.environ.get("FAKE_UV_FAIL"):
    sys.exit(3)
root = Path(os.environ["UV_PROJECT_ENVIRONMENT"])
(root / "bin").mkdir(parents=True, exist_ok=True)
(root / "pyvenv.cfg").write_text(
    "home = " + os.path.dirname({base!r}) + "\\ninclude-system-site-packages = false\\n",
    encoding="utf-8",
)
python = root / "bin" / "python"
if not python.exists():
    python.symlink_to({base!r})
"""


def make_checkout(path: Path, *, lock: str = "version = 1\n") -> Path:
    """A directory with the three lock inputs, pinned to this interpreter."""
    path.mkdir(parents=True, exist_ok=True)
    (path / "pyproject.toml").write_text(PYPROJECT, encoding="utf-8")
    (path / "uv.lock").write_text(lock, encoding="utf-8")
    (path / ".python-version").write_text(platform.python_version(), encoding="utf-8")
    return path


def fake_uv_environ(directory: Path) -> tuple[dict[str, str], Path]:
    """An environment naming a fake ``uv`` and an artifact root below
    ``directory``, and the file that logs the fake's calls."""
    if os.name == "nt":
        pytest.skip("the fake uv is a POSIX script")
    log = directory / "uv-calls.jsonl"
    uv = directory / "fake-bin" / "uv"
    uv.parent.mkdir(parents=True)
    uv.write_text(
        FAKE_UV.format(
            python=sys.executable,
            base=os.path.realpath(sys._base_executable),
            log=str(log),
        ),
        encoding="utf-8",
    )
    uv.chmod(0o755)
    return {
        "UV": str(uv),
        "PATH": os.environ.get("PATH", ""),
        "MOLT_EXT_ROOT": str(directory / "artifacts"),
    }, log


def uv_calls(log: Path) -> list[str]:
    return log.read_text(encoding="utf-8").splitlines() if log.exists() else []


def shared_environment(directory: Path, checkout: Path, artifacts: Path) -> Path:
    """The real shared environment of ``checkout`` under ``artifacts``, made
    with a fake ``uv`` below ``directory``; ``checkout`` gains lock inputs
    when it has none."""
    from molt import uv_environment

    if not (checkout / "uv.lock").is_file():
        make_checkout(checkout)
    fake = directory / f"fake-uv-{len(list(directory.glob('fake-uv-*')))}"
    fake.mkdir(parents=True)
    environ, _log = fake_uv_environ(fake)
    environ["MOLT_EXT_ROOT"] = str(artifacts)
    return uv_environment.environment(checkout, environ=environ)
