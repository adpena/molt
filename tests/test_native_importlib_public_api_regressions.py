from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

from tests.executable_test_support import native_executable_name
from tests.native_process_guard import run_native_test_process


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

ARTIFACT_ROOT = _HELPERS.ARTIFACT_ROOT
_build_native_binary_with_env = _HELPERS._build_native_binary_with_env
_build_and_run_with_env = _HELPERS._build_and_run_with_env


def test_native_importlib_public_api_validation_matches_cpython(
    tmp_path: Path,
) -> None:
    # Argument validation and resolve_name are pure operations. An ambient
    # developer grant must not hide a premature filesystem search.
    run = _build_and_run_with_env(
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
            "show('util-name-nonstr', lambda: importlib.util.resolve_name(1, None))\n"
            "show('util-relative-package-none', lambda: importlib.util.resolve_name('.x', None))\n"
            "show('util-relative-package-nonstr', lambda: importlib.util.resolve_name('.x', 1))\n"
            "show('util-relative-package-empty', lambda: importlib.util.resolve_name('.x', ''))\n"
            "show('util-beyond-top', lambda: importlib.util.resolve_name('..x', 'pkg'))\n"
            "show('util-empty-name', lambda: importlib.util.resolve_name('', None))\n"
            "show('util-relative-empty', lambda: importlib.util.resolve_name('.', 'pkg'))\n"
        ),
        native_executable_name("importlib_public_api_validation"),
        source_relpath="importlib_public_api_validation.py",
        session_id=_HELPERS.NATIVE_BOOTSTRAP_SESSION_ID,
        cache_dir=ARTIFACT_ROOT / ".molt_cache",
        backend="cranelift",
        extra_env={"MOLT_CAPABILITY_TIER": "none", "MOLT_CAPABILITIES": ""},
        extra_build_args=["--no-trusted"],
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
        "util-name-nonstr AttributeError 'int' object has no attribute 'startswith'",
        "util-relative-package-none ImportError no package specified for '.x' (required for relative module names)",
        "util-relative-package-nonstr AttributeError 'int' object has no attribute 'rsplit'",
        "util-relative-package-empty ImportError no package specified for '.x' (required for relative module names)",
        "util-beyond-top ImportError attempted relative import beyond top-level package",
        "util-empty-name OK ''",
        "util-relative-empty OK 'pkg'",
    ]


def test_native_importlib_discovery_preserves_capability_policy(tmp_path: Path) -> None:
    # CPython 3.12's _resolve_name('', package, 1) returns package; importing
    # that name then calls finders. Denial is Molt policy, not CPython absence.
    source = """import importlib
import sys

missing = '_molt_public_import_policy_absent_39c5b8'
try:
    importlib.import_module('.', missing)
except ImportError as exc:
    print('import-error', type(exc).__name__, str(exc))
except PermissionError as exc:
    print('permission-error', type(exc).__name__, str(exc))
print('missing-unpublished', missing not in sys.modules)

# The same name must import from the public cache without host discovery.
cached = object()
sys.modules[missing] = cached
print('cache-hit', importlib.import_module('.', missing) is cached)
"""
    manifest = tmp_path / "policy.toml"
    empty_path = tmp_path / "no-python-on-path"
    empty_path.mkdir()
    for policy, allowed in (
        ("allow = []\n", False),
        ('allow = ["fs.read"]\n', True),
        ('allow = ["fs.read"]\ndeny = ["fs.read"]\n', False),
    ):
        manifest.write_text(
            '[manifest]\nversion = "2.0"\n[capabilities]\n'
            + policy
            + '[audit]\nenabled = true\nsink = "jsonl"\noutput = "stderr"\n',
            encoding="utf-8",
        )
        # Same source, output and cache: changing policy reuses compiled code,
        # while the current manifest alone owns the emitted stub's grants.
        binary, env = _build_native_binary_with_env(
            tmp_path,
            source,
            native_executable_name("importlib_public_policy"),
            source_relpath="importlib_public_policy.py",
            session_id=_HELPERS.NATIVE_BOOTSTRAP_SESSION_ID,
            cache_dir=ARTIFACT_ROOT / ".molt_cache",
            backend="cranelift",
            extra_env={"MOLT_CAPABILITY_TIER": "none", "MOLT_CAPABILITIES": ""},
            extra_build_args=[
                "--no-trusted",
                "--cache",
                "--cache-dir",
                str(tmp_path / "code-cache"),
                "--capability-manifest",
                str(manifest),
            ],
        )
        # Adversarial ambient authority must not override the built policy.
        env.update(
            PATH=str(empty_path),
            PYTHONPATH="",
            PYTHONHOME="",
            MOLT_CAPABILITY_TIER="none" if allowed else "full",
            MOLT_CAPABILITIES="" if allowed else "fs.read",
        )
        run = run_native_test_process(
            [str(binary)],
            cwd=tmp_path,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        assert run.returncode == 0, policy + run.stdout + run.stderr
        missing = "_molt_public_import_policy_absent_39c5b8"
        discovery = (
            f"import-error ModuleNotFoundError No module named '{missing}'"
            if allowed
            else "permission-error PermissionError missing fs.read capability"
        )
        assert run.stdout.splitlines() == [
            discovery,
            "missing-unpublished True",
            "cache-hit True",
        ], policy + run.stdout + run.stderr
        decisions = [
            json.loads(line) for line in run.stderr.splitlines() if line.startswith("{")
        ]
        reads = [event for event in decisions if event["capability"] == "fs.read"]
        assert reads, policy + run.stderr
        assert all(
            event["decision"]["status"] == ("allowed" if allowed else "denied")
            for event in reads
        ), policy + run.stderr


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
        native_executable_name("importlib_import_module_joined_singleton_name"),
        source_relpath="importlib_import_module_joined_singleton_name.py",
        session_id="pytest-native-importlib-public-api-joined-name",
        cache_dir=ARTIFACT_ROOT / ".molt_cache",
        backend="cranelift",
        extra_env={"MOLT_CAPABILITY_TIER": "none", "MOLT_CAPABILITIES": ""},
        extra_build_args=[
            "--no-trusted",
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
