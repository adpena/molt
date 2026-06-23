from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


_HELPER_PATH = Path(__file__).with_name("test_native_import_bootstrap_regressions.py")
_HELPER_SPEC = importlib.util.spec_from_file_location(
    "_molt_native_import_bootstrap_helpers",
    _HELPER_PATH,
)
assert _HELPER_SPEC is not None
assert _HELPER_SPEC.loader is not None
_HELPERS = importlib.util.module_from_spec(_HELPER_SPEC)
sys.modules[_HELPER_SPEC.name] = _HELPERS
_HELPER_SPEC.loader.exec_module(_HELPERS)

ROOT = _HELPERS.ROOT
_build_and_run_with_env = _HELPERS._build_and_run_with_env


def test_native_from_import_star_all_auto_imports_package_child(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "from pkg import *\n"
            "import sys\n"
            "print(child.__name__)\n"
            "print(child.VALUE)\n"
            "print(child is sys.modules['pkg.child'])\n"
            "print(getattr(sys.modules['pkg'], 'child') is child)\n"
        ),
        "package_star_all_child",
        session_id="pytest-native-bootstrap-star-all-child",
        cache_dir=ROOT / ".molt_cache-package-star-all-child",
        backend="cranelift",
        source_relpath="main.py",
        extra_files={
            "pkg/__init__.py": "__all__ = ['child']\n",
            "pkg/child.py": "VALUE = 'loaded'\n",
        },
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "pkg.child",
        "loaded",
        "True",
        "True",
    ]


def test_native_from_import_star_all_missing_child_raises_attribute_error(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "try:\n"
            "    from pkg import *\n"
            "except BaseException as exc:\n"
            "    print(type(exc).__name__)\n"
            "    print(str(exc))\n"
            "else:\n"
            "    print('NO_ERROR')\n"
        ),
        "package_star_all_missing_child",
        session_id="pytest-native-bootstrap-star-all-missing-child",
        cache_dir=ROOT / ".molt_cache-package-star-all-missing-child",
        backend="cranelift",
        source_relpath="main.py",
        extra_files={"pkg/__init__.py": "__all__ = ['missing']\n"},
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "AttributeError",
        "module 'pkg' has no attribute 'missing'",
    ]
