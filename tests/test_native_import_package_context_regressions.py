from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


_HELPER_PATH = Path(__file__).with_name("test_native_import_bootstrap_regressions.py")
_HELPER_SPEC = importlib.util.spec_from_file_location(
    "_molt_native_import_bootstrap_helpers_for_context",
    _HELPER_PATH,
)
assert _HELPER_SPEC is not None
assert _HELPER_SPEC.loader is not None
_HELPERS = importlib.util.module_from_spec(_HELPER_SPEC)
sys.modules[_HELPER_SPEC.name] = _HELPERS
_HELPER_SPEC.loader.exec_module(_HELPERS)

ROOT = _HELPERS.ROOT
_build_and_run_with_env = _HELPERS._build_and_run_with_env


def test_native_dunder_import_relative_package_context_matches_cpython(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import builtins\n"
            "import pkg.helper as seeded_helper\n"
            "\n"
            "class Spec:\n"
            "    pass\n"
            "\n"
            "def show(label, func):\n"
            "    try:\n"
            "        value = func()\n"
            "    except BaseException as exc:\n"
            "        print(label, type(exc).__name__, str(exc))\n"
            "    else:\n"
            "        print(label, value.__name__, value is seeded_helper)\n"
            "\n"
            "spec = Spec()\n"
            "spec.parent = 'pkg'\n"
            "bad_spec = Spec()\n"
            "bad_spec.parent = 1\n"
            "missing_parent_spec = Spec()\n"
            "\n"
            "show('globals-none', lambda: builtins.__import__('helper', None, None, (), 1))\n"
            "show('globals-nondict', lambda: builtins.__import__('helper', 1, None, (), 1))\n"
            "show('package-nonstr', lambda: builtins.__import__('helper', {'__package__': 1}, None, (), 1))\n"
            "show('missing-name', lambda: builtins.__import__('helper', {}, None, (), 1))\n"
            "show('spec-parent', lambda: builtins.__import__('helper', {'__package__': None, '__spec__': spec}, None, ('ping',), 1))\n"
            "show('spec-parent-nonstr', lambda: builtins.__import__('helper', {'__package__': None, '__spec__': bad_spec}, None, (), 1))\n"
            "show('spec-parent-missing', lambda: builtins.__import__('helper', {'__package__': None, '__spec__': missing_parent_spec}, None, (), 1))\n"
            "show('name-fallback', lambda: builtins.__import__('helper', {'__name__': 'pkg.mod'}, None, ('ping',), 1))\n"
            "show('path-name', lambda: builtins.__import__('helper', {'__name__': 'pkg', '__path__': []}, None, ('ping',), 1))\n"
            "show('package-empty', lambda: builtins.__import__('helper', {'__package__': ''}, None, (), 1))\n"
        ),
        "dunder_import_package_context",
        session_id="pytest-native-bootstrap-import-package-context",
        cache_dir=ROOT / ".molt_cache-import-package-context",
        backend="cranelift",
        source_relpath="main.py",
        extra_files={
            "pkg/__init__.py": "",
            "pkg/helper.py": "def ping():\n    return 'ok'\n",
            "pkg/mod.py": "",
        },
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "globals-none TypeError globals must be a dict",
        "globals-nondict TypeError globals must be a dict",
        "package-nonstr TypeError package must be a string",
        "missing-name KeyError \"'__name__' not in globals\"",
        "spec-parent pkg.helper True",
        "spec-parent-nonstr TypeError __spec__.parent must be a string",
        "spec-parent-missing AttributeError 'Spec' object has no attribute 'parent'",
        "name-fallback pkg.helper True",
        "path-name pkg.helper True",
        "package-empty ImportError attempted relative import with no known parent package",
    ]
