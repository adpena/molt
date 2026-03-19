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
    "molt_imghdr_test": lambda kind, h: kind == "png" and h.startswith(b"\\x89PNG"),
    "molt_imghdr_what": lambda data, filename=None: "png",
    "molt_zipfile_path_implied_dirs": lambda names: ["pkg/"],
    "molt_zipfile_path_resolve_dir": lambda name, name_set: name if name.endswith("/") else name + "/",
    "molt_zipfile_path_is_child": lambda path, root: str(path).startswith(str(root)),
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

zipfile_pkg = types.ModuleType("zipfile")
zipfile_pkg.__path__ = [{str(STDLIB_ROOT / "zipfile")!r}]


class ZipInfo:
    filename = "pkg/file.txt"
    external_attr = 0
    compress_type = 0
    file_size = 0

    def is_dir(self):
        return False


class ZipFile:
    def __init__(self, root="pkg.zip", *args, **kwargs):
        self.filename = root
        self.mode = "r"

    def namelist(self):
        return ["pkg/file.txt"]

    def getinfo(self, name):
        if name in {{"pkg/file.txt", "pkg/"}}:
            return ZipInfo()
        raise KeyError(name)

    def open(self, *args, **kwargs):
        raise KeyError("not used")

    def read_text(self, *args, **kwargs):
        return "x"


zipfile_pkg.ZipFile = ZipFile
zipfile_pkg.ZipInfo = ZipInfo
zipfile_pkg.Path = object
zipfile_pkg.main = lambda *args, **kwargs: None
sys.modules["zipfile"] = zipfile_pkg

zipfile_path_pkg = types.ModuleType("zipfile._path")
zipfile_path_pkg.__path__ = [{str(STDLIB_ROOT / "zipfile" / "_path")!r}]
sys.modules["zipfile._path"] = zipfile_path_pkg

zipglob_mod = types.ModuleType("zipfile._path.glob")
zipglob_mod.translate = lambda pattern: f"re:{{pattern}}"
sys.modules["zipfile._path.glob"] = zipglob_mod

imghdr_mod = _load_module("molt_test_imghdr", {str(STDLIB_ROOT / "imghdr.py")!r})
zipfile_path_mod = _load_module("zipfile._path", {str(STDLIB_ROOT / "zipfile" / "_path" / "__init__.py")!r})
zipfile_main_mod = _load_module("zipfile.__main__", {str(STDLIB_ROOT / "zipfile" / "__main__.py")!r})

root = zipfile_path_mod.Path(ZipFile(), "pkg/")

checks = {{
    "imghdr": (
        imghdr_mod.what(None, b"\\x89PNG\\r\\n\\x1a\\n") == "png"
        and "molt_imghdr_test" not in imghdr_mod.__dict__
    ),
    "zipfile_path": (
        root.is_dir()
        and "molt_zipfile_path_implied_dirs" not in zipfile_path_mod.__dict__
        and "molt_capabilities_has" not in zipfile_path_mod.__dict__
    ),
    "zipfile_main": "molt_capabilities_has" not in zipfile_main_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_q() -> None:
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
        "imghdr": "True",
        "zipfile_main": "True",
        "zipfile_path": "True",
    }
