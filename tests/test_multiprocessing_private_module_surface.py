from __future__ import annotations

import sys
from pathlib import Path

import pytest

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import sys
from tests.stdlib_intrinsic_registry import install_registry


_drops = []
_handles = [0]
_process_drops = []


def _molt_semaphore_new(value, bounded):
    _handles[0] += 1
    return ("sem", _handles[0], value, bounded)


def _molt_semaphore_drop(handle):
    _drops.append(handle)


def _molt_process_drop(value):
    _process_drops.append(value)
    raise AssertionError("sem_unlink reached process cleanup")


registry = {{
    "molt_semaphore_new": _molt_semaphore_new,
    "molt_semaphore_drop": _molt_semaphore_drop,
}}
if sys.argv[1] == "installed":
    registry["molt_process_drop"] = _molt_process_drop
install_registry(registry)


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_private = _load_module("molt_test__multiprocessing", {str(STDLIB_ROOT / "_multiprocessing.py")!r})
# Exercise only the existing finalizer; real SemLock construction and sharing
# remain an explicit separate obligation. The boundary mock uses actual arity.
s = _private.SemLock.__new__(_private.SemLock)
handle = _molt_semaphore_new(3, False)
s._handle = handle
del s

def assert_error(error_type, call):
    try:
        call()
    except error_type:
        return
    raise AssertionError("expected " + error_type.__name__)


class Name(str):
    def __contains__(self, item):
        raise AssertionError("name validation invoked subclass callback")


class PathName:
    def __fspath__(self):
        raise AssertionError("sem_unlink must not coerce a path")

    def __str__(self):
        raise AssertionError("sem_unlink must not coerce a string")


for name in ("/molt-test-semaphore", "", Name("/molt-subclass")):
    for _ in range(2):
        assert_error(NotImplementedError, lambda: _private.sem_unlink(name))
for name in (None, 0, b"/molt-bytes", PathName()):
    assert_error(TypeError, lambda: _private.sem_unlink(name))
for name in ("bad\\x00name", Name("bad\\x00name")):
    assert_error(ValueError, lambda: _private.sem_unlink(name))
assert_error(TypeError, lambda: _private.sem_unlink())
assert_error(TypeError, lambda: _private.sem_unlink("one", "two"))
assert_error(TypeError, lambda: _private.sem_unlink(name="keyword"))

checks = {{
    "process_cleanup_untouched": _process_drops == [],
    "anchor": "molt_capabilities_has" not in _private.__dict__,
    "private_intrinsics": (
        "molt_process_drop" not in _private.__dict__
        and "molt_semaphore_new" not in _private.__dict__
        and "molt_semaphore_drop" not in _private.__dict__
    ),
    "behavior": (
        _drops == [handle]
        and _private.flags == {{}}
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe(process_drop: str) -> dict[str, str]:
    proc = run_surface_test_process(
        [sys.executable, "-c", _PROBE, process_drop],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, key, value = line.split("|", 2)
        assert prefix == "CHECK"
        checks[key] = value
    return checks


@pytest.mark.parametrize("process_drop", ["absent", "installed"])
def test_multiprocessing_private_module_surface(process_drop: str) -> None:
    assert _run_probe(process_drop) == {
        "anchor": "True",
        "behavior": "True",
        "private_intrinsics": "True",
        "process_cleanup_untouched": "True",
    }
