from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import re as _real_re
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
    "molt_re_literal_advance": lambda *args, **kwargs: 0,
    "molt_capabilities_has": lambda name: True,
    "molt_zipfile_path_translate_glob": lambda pattern, seps, recurse: f"rx:{{pattern}}:{{seps}}:{{recurse}}",
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

class _Parser:
    def __init__(self, pattern):
        self.pattern = pattern

    def parse(self):
        return ("parsed", self.pattern)


_real_re._Parser = _Parser
sys.modules["re"] = _real_re

zipfile_pkg = types.ModuleType("zipfile")
zipfile_pkg.__path__ = [{str(STDLIB_ROOT / "zipfile")!r}]
sys.modules["zipfile"] = zipfile_pkg
zipfile_path_pkg = types.ModuleType("zipfile._path")
zipfile_path_pkg.__path__ = [{str(STDLIB_ROOT / "zipfile" / "_path")!r}]
sys.modules["zipfile._path"] = zipfile_path_pkg

ctypes_pkg = types.ModuleType("ctypes")
ctypes_pkg.__path__ = [{str(STDLIB_ROOT / "ctypes")!r}]
sys.modules["ctypes"] = ctypes_pkg
macholib_pkg = types.ModuleType("ctypes.macholib")
macholib_pkg.__path__ = [{str(STDLIB_ROOT / "ctypes" / "macholib")!r}]
sys.modules["ctypes.macholib"] = macholib_pkg

parser_mod = _load_module("molt_test_re_parser", {str(STDLIB_ROOT / "re" / "_parser.py")!r})
casefix_mod = _load_module("molt_test_re_casefix", {str(STDLIB_ROOT / "re" / "_casefix.py")!r})
zipglob_mod = _load_module("zipfile._path.glob", {str(STDLIB_ROOT / "zipfile" / "_path" / "glob.py")!r})
macholib_init_mod = _load_module("ctypes.macholib", {str(STDLIB_ROOT / "ctypes" / "macholib" / "__init__.py")!r})
dylib_mod = _load_module("ctypes.macholib.dylib", {str(STDLIB_ROOT / "ctypes" / "macholib" / "dylib.py")!r})
framework_mod = _load_module("ctypes.macholib.framework", {str(STDLIB_ROOT / "ctypes" / "macholib" / "framework.py")!r})
dyld_mod = _load_module("ctypes.macholib.dyld", {str(STDLIB_ROOT / "ctypes" / "macholib" / "dyld.py")!r})

checks = {{
    "re_parser": (
        parser_mod.parse("ab") == ("parsed", "ab")
        and "molt_re_literal_advance" not in parser_mod.__dict__
    ),
    "re_casefix": (
        casefix_mod.EXTRA_CASES == {{}}
        and "molt_re_literal_advance" not in casefix_mod.__dict__
    ),
    "zipfile_path_glob": (
        zipglob_mod.translate("*.py") == "rx:*.py:/:False"
        and "molt_zipfile_path_translate_glob" not in zipglob_mod.__dict__
        and "molt_capabilities_has" not in zipglob_mod.__dict__
    ),
    "macholib_init": "molt_capabilities_has" not in macholib_init_mod.__dict__,
    "macholib_dylib": (
        dylib_mod.dylib_info("libfoo.dylib")["name"] == "libfoo"
        and "molt_capabilities_has" not in dylib_mod.__dict__
    ),
    "macholib_framework": (
        framework_mod.framework_info("/A/B.framework/B")["name"] == "B"
        and "molt_capabilities_has" not in framework_mod.__dict__
    ),
    "macholib_dyld": (
        dyld_mod.DEFAULT_LIBRARY_FALLBACK[-1] == "/usr/lib"
        and "molt_capabilities_has" not in dyld_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_l() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        if line.startswith("CHECK|"):
            _, key, value = line.split("|", 2)
            checks[key] = value
    assert checks == {
        "macholib_dyld": "True",
        "macholib_dylib": "True",
        "macholib_framework": "True",
        "macholib_init": "True",
        "re_casefix": "True",
        "re_parser": "True",
        "zipfile_path_glob": "True",
    }
