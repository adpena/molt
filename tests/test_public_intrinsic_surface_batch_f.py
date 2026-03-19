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


builtins._molt_intrinsics = {{
    "molt_capabilities_has": lambda name: True,
    "molt_glob_has_magic": lambda pathname: "*" in str(pathname),
    "molt_glob_escape": lambda pathname: str(pathname).replace("*", "[*]"),
    "molt_glob_glob": lambda pathname, root_dir, recursive: ["a.py", "b.py"],
    "molt_glob_iglob": lambda pathname, root_dir, recursive: ["a.py", "b.py"],
    "molt_path_isdir": lambda path: True,
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


annotationlib_mod = _load_module("molt_test_annotationlib", {str(STDLIB_ROOT / "annotationlib.py")!r})
getopt_mod = _load_module("molt_test_getopt", {str(STDLIB_ROOT / "getopt.py")!r})
mimetypes_mod = _load_module("molt_test_mimetypes", {str(STDLIB_ROOT / "mimetypes.py")!r})
numbers_mod = _load_module("molt_test_numbers", {str(STDLIB_ROOT / "numbers.py")!r})
glob_mod = _load_module("molt_test_glob", {str(STDLIB_ROOT / "glob.py")!r})

def _raises_runtimeerror(mod, attr):
    try:
        getattr(mod, attr)
    except RuntimeError:
        return True
    return False


checks = {{
    "annotationlib": (
        _raises_runtimeerror(annotationlib_mod, "X")
        and "molt_capabilities_has" not in annotationlib_mod.__dict__
    ),
    "getopt": (
        _raises_runtimeerror(getopt_mod, "gnu_getopt")
        and "molt_capabilities_has" not in getopt_mod.__dict__
    ),
    "mimetypes": (
        _raises_runtimeerror(mimetypes_mod, "guess_type")
        and "molt_capabilities_has" not in mimetypes_mod.__dict__
    ),
    "numbers": (
        _raises_runtimeerror(numbers_mod, "Number")
        and "molt_capabilities_has" not in numbers_mod.__dict__
    ),
    "glob": (
        glob_mod.has_magic("*.py") is True
        and glob_mod.escape("*.py") == "[*].py"
        and list(glob_mod.iglob("*.py")) == ["a.py", "b.py"]
        and "molt_glob_glob" not in glob_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_f() -> None:
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
        "annotationlib": "True",
        "getopt": "True",
        "glob": "True",
        "mimetypes": "True",
        "numbers": "True",
    }
