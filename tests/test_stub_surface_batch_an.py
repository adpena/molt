from __future__ import annotations

import builtins
import importlib.util
import sys
import types
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
UNITTEST_STUBS = [
    ROOT / "src/molt/stdlib/unittest/__main__.py",
    ROOT / "src/molt/stdlib/unittest/_log.py",
    ROOT / "src/molt/stdlib/unittest/async_case.py",
    ROOT / "src/molt/stdlib/unittest/case.py",
    ROOT / "src/molt/stdlib/unittest/loader.py",
    ROOT / "src/molt/stdlib/unittest/main.py",
    ROOT / "src/molt/stdlib/unittest/mock.py",
    ROOT / "src/molt/stdlib/unittest/result.py",
    ROOT / "src/molt/stdlib/unittest/runner.py",
    ROOT / "src/molt/stdlib/unittest/signals.py",
    ROOT / "src/molt/stdlib/unittest/suite.py",
    ROOT / "src/molt/stdlib/unittest/util.py",
]
SQLITE3_STUBS = [
    ROOT / "src/molt/stdlib/sqlite3/__init__.py",
    ROOT / "src/molt/stdlib/sqlite3/__main__.py",
    ROOT / "src/molt/stdlib/sqlite3/dbapi2.py",
]
TEST_SUPPORT_PACKAGE = ROOT / "src/molt/stdlib/test/support"


def _install_intrinsics() -> tuple[types.ModuleType | None, object]:
    previous_intrinsics_mod = sys.modules.get("_intrinsics")
    previous_builtins = getattr(builtins, "_molt_intrinsics", None)
    builtins._molt_intrinsics = {"molt_capabilities_has": lambda name: True}

    intrinsics_mod = types.ModuleType("_intrinsics")

    def require_intrinsic(name: str, namespace: dict[str, object] | None = None):
        value = builtins._molt_intrinsics.get(name)
        if value is None:
            raise RuntimeError(f"missing intrinsic: {name}")
        if namespace is not None:
            namespace[name] = value
        return value

    intrinsics_mod.require_intrinsic = require_intrinsic
    sys.modules["_intrinsics"] = intrinsics_mod
    return previous_intrinsics_mod, previous_builtins


def _restore_intrinsics(
    previous_intrinsics_mod: types.ModuleType | None, previous_builtins: object
) -> None:
    if previous_intrinsics_mod is None:
        sys.modules.pop("_intrinsics", None)
    else:
        sys.modules["_intrinsics"] = previous_intrinsics_mod
    if previous_builtins is None:
        del builtins._molt_intrinsics
    else:
        builtins._molt_intrinsics = previous_builtins


def _load_module(path: Path, module_name: str) -> types.ModuleType:
    spec = importlib.util.spec_from_file_location(module_name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    try:
        spec.loader.exec_module(module)
    except Exception:
        sys.modules.pop(module_name, None)
        raise
    return module


def test_unittest_sqlite3_and_test_support_hide_raw_capability_intrinsic() -> None:
    previous_intrinsics_mod, previous_builtins = _install_intrinsics()
    previous_modules = {
        name: sys.modules.get(name)
        for name in [
            "test",
            "test.support",
            "test.support._fallback_support",
            "test.support.os_helper",
            "test.support.import_helper",
            "test.support.warnings_helper",
        ]
    }
    try:
        for index, path in enumerate(UNITTEST_STUBS):
            module = _load_module(
                path, f"_molt_test_stub_surface_batch_an_unittest_{index}"
            )
            assert "molt_capabilities_has" not in module.__dict__
            assert "_MOLT_CAPABILITIES_HAS" in module.__dict__
            try:
                getattr(module, "sentinel")
            except RuntimeError as exc:
                assert "only an intrinsic-first stub is available" in str(exc)
            else:
                raise AssertionError(
                    f"{path} did not raise RuntimeError from __getattr__"
                )

        for index, path in enumerate(SQLITE3_STUBS):
            module = _load_module(
                path, f"_molt_test_stub_surface_batch_an_sqlite3_{index}"
            )
            assert "molt_capabilities_has" not in module.__dict__
            assert "_MOLT_CAPABILITIES_HAS" in module.__dict__
            try:
                getattr(module, "sentinel")
            except RuntimeError as exc:
                assert "only an intrinsic-first stub is available" in str(exc)
            else:
                raise AssertionError(
                    f"{path} did not raise RuntimeError from __getattr__"
                )

        test_pkg = types.ModuleType("test")
        test_pkg.__path__ = [str(TEST_SUPPORT_PACKAGE.parent)]
        support_pkg = types.ModuleType("test.support")
        support_pkg.__path__ = [str(TEST_SUPPORT_PACKAGE)]
        sys.modules["test"] = test_pkg
        sys.modules["test.support"] = support_pkg

        fallback = _load_module(
            TEST_SUPPORT_PACKAGE / "_fallback_support.py",
            "test.support._fallback_support",
        )
        os_helper = _load_module(
            TEST_SUPPORT_PACKAGE / "os_helper.py", "test.support.os_helper"
        )
        import_helper = _load_module(
            TEST_SUPPORT_PACKAGE / "import_helper.py", "test.support.import_helper"
        )
        support = _load_module(TEST_SUPPORT_PACKAGE / "__init__.py", "test.support")
        warnings_helper = _load_module(
            TEST_SUPPORT_PACKAGE / "warnings_helper.py", "test.support.warnings_helper"
        )

        for module in [fallback, os_helper, import_helper, support, warnings_helper]:
            assert "molt_capabilities_has" not in module.__dict__
            assert "_MOLT_CAPABILITIES_HAS" in module.__dict__
    finally:
        for name, module in previous_modules.items():
            if module is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = module
        _restore_intrinsics(previous_intrinsics_mod, previous_builtins)
