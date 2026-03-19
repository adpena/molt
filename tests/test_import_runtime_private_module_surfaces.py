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


def _frozen_payload(machinery, util):
    return {{
        "BuiltinImporter": machinery.BuiltinImporter,
        "FrozenImporter": machinery.FrozenImporter,
        "ModuleSpec": machinery.ModuleSpec,
        "module_from_spec": util.module_from_spec,
        "spec_from_loader": util.spec_from_loader,
    }}


def _frozen_external_payload(machinery, util):
    _file_loader = getattr(machinery, "FileLoader", machinery.SourceFileLoader)
    _source_loader = getattr(machinery, "SourceLoader", machinery.SourceFileLoader)
    return {{
        "BYTECODE_SUFFIXES": [".pyc"],
        "DEBUG_BYTECODE_SUFFIXES": [".pyc"],
        "EXTENSION_SUFFIXES": [".so"],
        "MAGIC_NUMBER": b"\\x01\\x02\\x03\\x04",
        "OPTIMIZED_BYTECODE_SUFFIXES": [".opt.pyc"],
        "SOURCE_SUFFIXES": [".py"],
        "ExtensionFileLoader": machinery.ExtensionFileLoader,
        "FileFinder": machinery.FileFinder,
        "FileLoader": _file_loader,
        "NamespaceLoader": machinery.NamespaceLoader,
        "PathFinder": machinery.PathFinder,
        "SourceFileLoader": machinery.SourceFileLoader,
        "SourceLoader": _source_loader,
        "SourcelessFileLoader": machinery.SourcelessFileLoader,
        "_LoaderBasics": machinery.SourceFileLoader,
        "WindowsRegistryFinder": getattr(machinery, "WindowsRegistryFinder", type("WindowsRegistryFinder", (), {{}})),
        "cache_from_source": util.cache_from_source,
        "decode_source": lambda data: data.decode() if isinstance(data, bytes) else str(data),
        "source_from_cache": util.source_from_cache,
        "spec_from_file_location": util.spec_from_file_location,
    }}


builtins._molt_intrinsics = {{
    "molt_capabilities_has": lambda _name=None: True,
    "molt_importlib_frozen_payload": _frozen_payload,
    "molt_importlib_frozen_external_payload": _frozen_external_payload,
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


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_frozen = _load_module("molt_test__frozen_importlib", {str(STDLIB_ROOT / "_frozen_importlib.py")!r})
_external = _load_module("molt_test__frozen_importlib_external", {str(STDLIB_ROOT / "_frozen_importlib_external.py")!r})

checks = {{
    "frozen_anchor": "molt_capabilities_has" not in _frozen.__dict__,
    "frozen_payload_anchor": "molt_importlib_frozen_payload" not in _frozen.__dict__,
    "frozen_behavior": hasattr(_frozen, "BuiltinImporter") and hasattr(_frozen, "spec_from_loader"),
    "external_anchor": "molt_capabilities_has" not in _external.__dict__,
    "external_payload_anchor": "molt_importlib_frozen_external_payload" not in _external.__dict__,
    "external_behavior": (
        _external.BYTECODE_SUFFIXES == [".pyc"]
        and _external.SOURCE_SUFFIXES == [".py"]
        and _external.path_sep == "/"
        and _external.path_sep_tuple == ("/", "\\\\")
        and _external.path_separators == "/\\\\"
        and hasattr(_external, "spec_from_file_location")
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> dict[str, str]:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
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


def test_import_runtime_private_module_surfaces() -> None:
    assert _run_probe() == {
        "external_anchor": "True",
        "external_behavior": "True",
        "external_payload_anchor": "True",
        "frozen_anchor": "True",
        "frozen_behavior": "True",
        "frozen_payload_anchor": "True",
    }
