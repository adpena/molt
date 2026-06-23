from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


_HELPER_PATH = Path(__file__).with_name("test_native_import_bootstrap_regressions.py")
_HELPER_SPEC = importlib.util.spec_from_file_location(
    "_molt_native_import_bootstrap_helpers_for_public_api",
    _HELPER_PATH,
)
assert _HELPER_SPEC is not None
assert _HELPER_SPEC.loader is not None
_HELPERS = importlib.util.module_from_spec(_HELPER_SPEC)
sys.modules[_HELPER_SPEC.name] = _HELPERS
_HELPER_SPEC.loader.exec_module(_HELPERS)

ROOT = _HELPERS.ROOT
_build_and_run = _HELPERS._build_and_run
_build_and_run_with_env = _HELPERS._build_and_run_with_env


def test_native_importlib_public_api_validation_matches_cpython(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import importlib\n"
            "import importlib.util\n"
            "\n"
            "def show(label, func):\n"
            "    try:\n"
            "        value = func()\n"
            "    except BaseException as exc:\n"
            "        print(label, type(exc).__name__, str(exc))\n"
            "    else:\n"
            "        print(label, 'OK', repr(value))\n"
            "\n"
            "show('import-module-name-nonstr', lambda: importlib.import_module(1))\n"
            "show('import-module-relative-package-none', lambda: importlib.import_module('.x', None))\n"
            "show('import-module-relative-package-missing', lambda: importlib.import_module('.x'))\n"
            "show('import-module-relative-package-nonstr', lambda: importlib.import_module('.x', 1))\n"
            "show('import-module-relative-package-empty', lambda: importlib.import_module('.x', ''))\n"
            "show('import-module-beyond-top', lambda: importlib.import_module('..x', 'pkg'))\n"
            "show('import-module-empty-name', lambda: importlib.import_module(''))\n"
            "show('import-module-relative-empty', lambda: importlib.import_module('.', 'pkg'))\n"
            "show('util-name-nonstr', lambda: importlib.util.resolve_name(1, None))\n"
            "show('util-relative-package-none', lambda: importlib.util.resolve_name('.x', None))\n"
            "show('util-relative-package-nonstr', lambda: importlib.util.resolve_name('.x', 1))\n"
            "show('util-relative-package-empty', lambda: importlib.util.resolve_name('.x', ''))\n"
            "show('util-beyond-top', lambda: importlib.util.resolve_name('..x', 'pkg'))\n"
            "show('util-empty-name', lambda: importlib.util.resolve_name('', None))\n"
            "show('util-relative-empty', lambda: importlib.util.resolve_name('.', 'pkg'))\n"
        ),
        "importlib_public_api_validation",
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "import-module-name-nonstr AttributeError 'int' object has no attribute 'startswith'",
        "import-module-relative-package-none TypeError the 'package' argument is required to perform a relative import for '.x'",
        "import-module-relative-package-missing TypeError the 'package' argument is required to perform a relative import for '.x'",
        "import-module-relative-package-nonstr TypeError __package__ not set to a string",
        "import-module-relative-package-empty TypeError the 'package' argument is required to perform a relative import for '.x'",
        "import-module-beyond-top ImportError attempted relative import beyond top-level package",
        "import-module-empty-name ValueError Empty module name",
        "import-module-relative-empty ModuleNotFoundError No module named 'pkg'",
        "util-name-nonstr AttributeError 'int' object has no attribute 'startswith'",
        "util-relative-package-none ImportError no package specified for '.x' (required for relative module names)",
        "util-relative-package-nonstr AttributeError 'int' object has no attribute 'rsplit'",
        "util-relative-package-empty ImportError no package specified for '.x' (required for relative module names)",
        "util-beyond-top ImportError attempted relative import beyond top-level package",
        "util-empty-name OK ''",
        "util-relative-empty OK 'pkg'",
    ]


def test_native_importlib_import_module_joined_singleton_name_returns_module(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import importlib\n"
            "\n"
            "def _module_name(parts: tuple[str, ...]) -> str:\n"
            "    return '.'.join(parts)\n"
            "\n"
            "def _load(parts: tuple[str, ...]):\n"
            "    return importlib.import_module(_module_name(parts))\n"
            "\n"
            "math_mod = _load(('math',))\n"
            "util_mod = _load(('importlib', 'util'))\n"
            "print(math_mod.__name__)\n"
            "print(util_mod.__name__)\n"
            "print(math_mod is importlib.import_module('math'))\n"
            "print(util_mod is importlib.import_module('importlib.util'))\n"
        ),
        "importlib_import_module_joined_singleton_name",
        session_id="pytest-native-importlib-public-api-joined-name",
        cache_dir=ROOT / ".molt_cache-importlib-public-api-joined-name",
        backend="cranelift",
        extra_build_args=[
            "--stdlib-profile",
            "full",
        ],
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "math",
        "importlib.util",
        "True",
        "True",
    ]
