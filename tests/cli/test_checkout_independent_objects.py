"""Two checkouts of the same compiler sources build the same objects (HF-372).

The test copies this checkout's source files to a second root, builds each
program with each checkout's own compiler and backend, and compares the native
object bytes. The stdlib program embeds a stdlib module's paths, so a checkout
path in any emitted location changes its object. The design is
docs/spec/areas/core/0025_REPRODUCIBLE_AND_DETERMINISTIC_MODE.md, section 2.3.

The second checkout compiles its own backend into a private Cargo target. It is
slow, so it runs deliberately (``-m slow``) and in the native integration cell.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import sys
from pathlib import Path

import pytest

from molt.backend_environment import environment_keys
from tests import process_guard_common
from tests.native_process_guard import run_native_test_process

pytestmark = pytest.mark.slow

ROOT = Path(__file__).resolve().parents[2]

PROGRAMS = {
    "hello": 'print("hello")\n',
    "stdlib": (
        "import json\n"
        "import os\n"
        "print(json.dumps({'tail': json.__file__.split('/')[-2:]}))\n"
        "print(os.path.basename(__file__))\n"
    ),
}


def _copy_checkout(destination: Path) -> None:
    """Copy the tracked and unignored files, as a second checkout holds them."""
    listed = process_guard_common.run_guarded_test_process(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        cwd=ROOT,
        timeout=120,
        text=False,
    )
    assert listed.returncode == 0, listed.stderr
    for raw in listed.stdout.split(b"\0"):
        if not raw:
            continue
        relative = os.fsdecode(raw)
        source = ROOT / relative
        if not source.exists() and not source.is_symlink():
            continue  # Deleted in the working tree, not yet in the index.
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target, follow_symlinks=False)


def _build(checkout: Path, program: Path, output: Path, scratch: Path) -> dict:
    environment = dict(os.environ)
    for name in environment_keys("diagnostic"):
        environment.pop(name, None)
    environment.pop("MOLT_SOURCE_ROOT", None)
    environment.update(
        PYTHONPATH=str(checkout / "src"),
        MOLT_BACKEND_DAEMON="0",
        # Separate caches: no object crosses from one checkout to the other.
        MOLT_CACHE=str(scratch / f"cache-{checkout.name}"),
    )
    if checkout != ROOT:
        environment["CARGO_TARGET_DIR"] = str(scratch / "target-second-checkout")
    built = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--build-profile",
            "dev",
            "--emit",
            "obj",
            "--json",
            "--output",
            str(output),
            str(program),
        ],
        cwd=checkout,
        env=environment,
        timeout=3600,
    )
    assert built.returncode == 0, f"stdout:\n{built.stdout}\nstderr:\n{built.stderr}"
    return json.loads(built.stdout.strip().splitlines()[-1])["data"]


def test_two_checkouts_build_byte_identical_objects() -> None:
    with process_guard_common.guarded_temporary_directory(
        prefix="molt-checkout-objects-"
    ) as scratch:
        second = scratch / "second-checkout"
        _copy_checkout(second)
        programs = scratch / "programs"
        programs.mkdir()
        compilers: dict[Path, set[str]] = {ROOT: set(), second: set()}
        for name, text in PROGRAMS.items():
            program = programs / f"{name}.py"
            program.write_text(text, encoding="utf-8")
            objects: dict[Path, bytes] = {}
            for checkout in (ROOT, second):
                output = scratch / f"{name}-{checkout.name}.o"
                result = _build(checkout, program, output, scratch)
                compilers[checkout].add(
                    result["observed_toolchain"]["compiler"]["path"]
                )
                objects[checkout] = output.read_bytes()
            digests = {
                checkout.name: hashlib.sha256(content).hexdigest()
                for checkout, content in objects.items()
            }
            print(f"checkout-object {name} {json.dumps(digests)}", flush=True)
            for checkout, content in objects.items():
                for root in (ROOT, second):
                    assert os.fsencode(root) not in content, (
                        f"{name} built by {checkout} embeds checkout path {root}"
                    )
            assert objects[ROOT] == objects[second], digests
            if name == "stdlib":
                assert b"/__molt__/stdlib/json/__init__.py" in objects[ROOT]
                assert os.fsencode(program.resolve()) in objects[ROOT]
        # Each checkout ran the compiler it built, not one shared executable.
        assert compilers[ROOT].isdisjoint(compilers[second]), compilers
