from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import sys
import types


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


class _FakeBuffer:
    def __init__(self, text, binary):
        self._binary = binary
        if binary:
            self._lines = [line.encode("utf-8") for line in text.splitlines(True)]
        else:
            self._lines = text.splitlines(True)
        self.name = "foo.py"

    def readline(self):
        if self._lines:
            return self._lines.pop(0)
        return b"" if self._binary else ""

    def readlines(self):
        lines = list(self._lines)
        self._lines.clear()
        return lines

    def close(self):
        return None


def _file_open_ex(path, mode, buffering, encoding, errors, newline, closefd, opener):
    del path, buffering, errors, newline, closefd, opener
    text = "first\\nsecond\\n"
    return _FakeBuffer(text, "b" in mode)


builtins._molt_intrinsics = {{
    "molt_stdlib_probe": lambda: None,
    "molt_file_open_ex": _file_open_ex,
    "molt_path_exists": lambda path: path == "foo.py",
    "molt_path_isabs": lambda path: False,
    "molt_path_join": lambda dirname, filename: f"{{dirname}}/{{filename}}",
    "molt_linecache_loader_get_source": lambda loader, name: None,
    "molt_linecache_detect_encoding": lambda first, second: ("utf-8", False),
    "molt_capabilities_has": lambda name: True,
}}

_intrinsics_mod = types.ModuleType("_intrinsics")


def _require_intrinsic(name, namespace=None):
    intrinsics = getattr(builtins, "_molt_intrinsics", {{}})
    if name in intrinsics:
        value = intrinsics[name]
        if namespace is not None:
            namespace[name] = value
        return value
    raise RuntimeError(f"intrinsic unavailable: {{name}}")


_intrinsics_mod.require_intrinsic = _require_intrinsic
sys.modules["_intrinsics"] = _intrinsics_mod


linecache_mod = _load_module("molt_test_linecache", {str(STDLIB_ROOT / "linecache.py")!r})
sndhdr_mod = _load_module("molt_test_sndhdr", {str(STDLIB_ROOT / "sndhdr.py")!r})
winsound_mod = _load_module("molt_test_winsound", {str(STDLIB_ROOT / "winsound.py")!r})
wsgiref_types_mod = _load_module("molt_test_wsgiref_types", {str(STDLIB_ROOT / "wsgiref" / "types.py")!r})
wsgiref_validate_mod = _load_module("molt_test_wsgiref_validate", {str(STDLIB_ROOT / "wsgiref" / "validate.py")!r})


def _raises_runtimeerror(mod, attr):
    try:
        getattr(mod, attr)
    except RuntimeError:
        return True
    return False


checks = {{
    "linecache": (
        linecache_mod.getline("foo.py", 2) == "second\\n"
        and "molt_file_open_ex" not in linecache_mod.__dict__
        and "molt_stdlib_probe" not in linecache_mod.__dict__
    ),
    "sndhdr": (
        _raises_runtimeerror(sndhdr_mod, "whathdr")
        and "molt_capabilities_has" not in sndhdr_mod.__dict__
    ),
    "winsound": (
        _raises_runtimeerror(winsound_mod, "Beep")
        and "molt_capabilities_has" not in winsound_mod.__dict__
    ),
    "wsgiref_types": (
        _raises_runtimeerror(wsgiref_types_mod, "InputStream")
        and "molt_capabilities_has" not in wsgiref_types_mod.__dict__
    ),
    "wsgiref_validate": (
        _raises_runtimeerror(wsgiref_validate_mod, "validator")
        and "molt_capabilities_has" not in wsgiref_validate_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_h() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "CHECK":
            checks[rest[0]] = rest[1]
    assert checks == {
        "linecache": "True",
        "sndhdr": "True",
        "winsound": "True",
        "wsgiref_types": "True",
        "wsgiref_validate": "True",
    }
