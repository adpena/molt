from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import types as _types
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
    "molt_import_smoke_runtime_ready": lambda: None,
    "molt_token_payload_312": lambda: {{
        "_payload_schema": "molt.token_payload.312.v1",
        "_python_minor": "3.12",
        "constants": {{"ENDMARKER": 0, "NAME": 1, "NT_OFFSET": 256}},
        "constant_order": ["ENDMARKER", "NAME", "NT_OFFSET"],
        "tok_name": {{"0": "ENDMARKER", "1": "NAME", "256": "NT_OFFSET"}},
        "EXACT_TOKEN_TYPES": {{"+": 1}},
    }},
    "molt_this_payload": lambda: ("enc", {{"a": "b"}}, "Zen line", 1, 2),
    "molt_importlib_find_in_path_package_context": lambda fullname, roots: {{
        "loader_kind": "zip_source",
        "zip_archive": "pkg.zip",
        "zip_inner_path": "pkg/mod.py",
        "is_package": False,
        "origin": "pkg.zip/pkg/mod.py",
    }},
    "molt_importlib_zip_source_exec_payload": lambda fullname, archive, inner, is_package: {{
        "source": "VALUE = 7\\n"
    }},
    "molt_importlib_zip_read_entry": lambda archive, inner: b"payload",
    "molt_capabilities_trusted": lambda: True,
    "molt_capabilities_require": lambda cap: None,
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

machinery_mod = _types.ModuleType("importlib.machinery")


class ModuleSpec:
    def __init__(self, name, loader=None, origin=None, is_package=False):
        self.name = name
        self.loader = loader
        self.origin = origin
        self.submodule_search_locations = [] if is_package else None


class _ZipSourceLoader:
    def __init__(self, fullname, archive, inner):
        self.fullname = fullname
        self.archive = archive
        self.inner = inner

    def exec_module(self, module):
        module.VALUE = 7


machinery_mod.ModuleSpec = ModuleSpec
machinery_mod._ZipSourceLoader = _ZipSourceLoader
sys.modules["importlib.machinery"] = machinery_mod


token_mod = _load_module("molt_test_token", {str(STDLIB_ROOT / "token.py")!r})
this_mod = _load_module("molt_test_this", {str(STDLIB_ROOT / "this.py")!r})
zipimport_mod = _load_module("molt_test_zipimport", {str(STDLIB_ROOT / "zipimport.py")!r})

imported = zipimport_mod.zipimporter("pkg.zip")
spec = imported.find_spec("pkg.mod")
data = imported.get_data("pkg.zip/pkg/mod.py")

checks = {{
    "token": (
        token_mod.ENDMARKER == 0
        and token_mod.EXACT_TOKEN_TYPES["+"] == 1
        and "molt_token_payload_312" not in token_mod.__dict__
    ),
    "this": (
        getattr(this_mod, "s") == "enc"
        and "molt_this_payload" not in this_mod.__dict__
    ),
    "zipimport": (
        spec is not None
        and imported.get_source("pkg.mod") == "VALUE = 7\\n"
        and data == b"payload"
        and "molt_importlib_zip_read_entry" not in zipimport_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_k() -> None:
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
        "this": "True",
        "token": "True",
        "zipimport": "True",
    }
