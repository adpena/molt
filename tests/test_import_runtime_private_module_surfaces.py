from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.machinery
import importlib.util
import sys
from tests.stdlib_intrinsic_registry import install_registry

_payload_attempts = []
_committed_namespaces = []
_payload_failure = RuntimeError("external payload failed")
_body_failure = RuntimeError("external assignment failed")


class _FailingAssignmentPayload(dict):
    def __getitem__(self, name):
        if name == "WindowsRegistryFinder":
            raise _body_failure
        return super().__getitem__(name)


def _frozen_payload():
    machinery = importlib.machinery
    return {{
        "BuiltinImporter": machinery.BuiltinImporter,
        "FrozenImporter": machinery.FrozenImporter,
        "ModuleSpec": machinery.ModuleSpec,
        "_DeadlockError": importlib._bootstrap._DeadlockError,
    }}


def _frozen_external_payload(machinery, namespace):
    assert namespace["_machinery"] is machinery
    assert namespace["_load_payload"].__globals__ is namespace
    assert "SOURCE_SUFFIXES" not in namespace
    assert "BYTECODE_SUFFIXES" not in namespace
    _payload_attempts.append(namespace)
    if len(_payload_attempts) == 1:
        raise _payload_failure
    _file_loader = getattr(machinery, "FileLoader", machinery.SourceFileLoader)
    _source_loader = getattr(machinery, "SourceLoader", machinery.SourceFileLoader)
    payload = {{
        "BYTECODE_SUFFIXES": machinery.BYTECODE_SUFFIXES,
        "DEBUG_BYTECODE_SUFFIXES": machinery.DEBUG_BYTECODE_SUFFIXES,
        "EXTENSION_SUFFIXES": [".so"],
        "MAGIC_NUMBER": b"\\x01\\x02\\x03\\x04",
        "OPTIMIZED_BYTECODE_SUFFIXES": machinery.OPTIMIZED_BYTECODE_SUFFIXES,
        "SOURCE_SUFFIXES": machinery.SOURCE_SUFFIXES,
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
    }}
    if len(_payload_attempts) == 2:
        return _FailingAssignmentPayload(payload)
    return payload


def _frozen_external_commit(namespace):
    assert "_PAYLOAD" not in namespace
    assert "_require_intrinsic" not in namespace
    assert namespace["SOURCE_SUFFIXES"] is namespace["_machinery"].SOURCE_SUFFIXES
    assert namespace["BYTECODE_SUFFIXES"] is namespace["_machinery"].BYTECODE_SUFFIXES
    assert all(name in namespace for name in namespace["__all__"])
    assert namespace["cache_from_source"].__globals__ is namespace
    assert namespace["spec_from_file_location"].__globals__ is namespace
    _committed_namespaces.append(namespace)


install_registry({{
    "molt_importlib_frozen_payload": _frozen_payload,
    "molt_importlib_frozen_external_payload": _frozen_external_payload,
    "molt_importlib_frozen_external_commit": _frozen_external_commit,
    "molt_importlib_module_from_spec": importlib.util.module_from_spec,
    "molt_importlib_spec_from_loader": importlib.util.spec_from_loader,
    "molt_importlib_find_spec": importlib._bootstrap._find_spec,
    "molt_importlib_cache_from_source": importlib.util.cache_from_source,
    "molt_importlib_decode_source": lambda data: data.decode() if isinstance(data, bytes) else str(data),
    "molt_importlib_source_from_cache": importlib.util.source_from_cache,
    "molt_importlib_spec_from_file_location": (
        lambda name, location, loader, submodule_search_locations, machinery:
        importlib.util.spec_from_file_location(
            name,
            location,
            loader=loader,
            submodule_search_locations=submodule_search_locations,
        )
    ),
}})


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    try:
        spec.loader.exec_module(module)
    except BaseException:
        sys.modules.pop(name, None)
        raise
    return module


_frozen = _load_module("molt_test__frozen_importlib", {str(STDLIB_ROOT / "_frozen_importlib.py")!r})
for failure in (_payload_failure, _body_failure):
    try:
        _load_module("molt_test__frozen_importlib_external", {str(STDLIB_ROOT / "_frozen_importlib_external.py")!r})
    except RuntimeError as error:
        assert error is failure
    else:
        raise AssertionError("external initialization must fail")
    assert not _committed_namespaces
    assert "molt_test__frozen_importlib_external" not in sys.modules

assert "SOURCE_SUFFIXES" not in _payload_attempts[0]
assert "BYTECODE_SUFFIXES" not in _payload_attempts[0]
assert "SOURCE_SUFFIXES" in _payload_attempts[1]
assert "ExtensionFileLoader" in _payload_attempts[1]
assert "WindowsRegistryFinder" not in _payload_attempts[1]
_external = _load_module("molt_test__frozen_importlib_external", {str(STDLIB_ROOT / "_frozen_importlib_external.py")!r})

checks = {{
    "frozen_anchor": "molt_capabilities_has" not in _frozen.__dict__,
    "frozen_payload_anchor": "molt_importlib_frozen_payload" not in _frozen.__dict__,
    "frozen_behavior": hasattr(_frozen, "BuiltinImporter") and hasattr(_frozen, "spec_from_loader"),
    "external_anchor": "molt_capabilities_has" not in _external.__dict__,
    "external_payload_anchor": "molt_importlib_frozen_external_payload" not in _external.__dict__,
    "external_failure_atomic": (
        len(_payload_attempts) == 3
        and len(_committed_namespaces) == 1
        and _committed_namespaces[0] is _external.__dict__
        and _payload_attempts[0] is not _external.__dict__
        and _payload_attempts[1] is not _external.__dict__
    ),
    "external_behavior": (
        _external.BYTECODE_SUFFIXES == [".pyc"]
        and _external.SOURCE_SUFFIXES is importlib.machinery.SOURCE_SUFFIXES
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
    proc = run_surface_test_process(
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
        "external_failure_atomic": "True",
        "external_payload_anchor": "True",
        "frozen_anchor": "True",
        "frozen_behavior": "True",
        "frozen_payload_anchor": "True",
    }
