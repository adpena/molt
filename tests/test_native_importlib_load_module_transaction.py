from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


_HELPER_PATH = Path(__file__).with_name("test_native_import_bootstrap_regressions.py")
_HELPER_SPEC = importlib.util.spec_from_file_location(
    "_molt_native_import_bootstrap_helpers_for_load_module_transaction",
    _HELPER_PATH,
)
assert _HELPER_SPEC is not None
assert _HELPER_SPEC.loader is not None
_HELPERS = importlib.util.module_from_spec(_HELPER_SPEC)
sys.modules[_HELPER_SPEC.name] = _HELPERS
_HELPER_SPEC.loader.exec_module(_HELPERS)

ROOT = _HELPERS.ROOT
_build_and_run_with_env = _HELPERS._build_and_run_with_env


def test_native_sourcefileloader_load_module_transaction_matches_cpython(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "runtime_site" / "loader_target.py"
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import importlib.machinery\n"
            "import sys\n"
            "import types\n"
            f"path = {str(module_path)!r}\n"
            "\n"
            "def show(label, func):\n"
            "    try:\n"
            "        value = func()\n"
            "    except BaseException as exc:\n"
            "        print(label, type(exc).__name__, str(exc))\n"
            "    else:\n"
            "        print(label, 'OK', repr(value))\n"
            "\n"
            "class BoomLoader(importlib.machinery.SourceFileLoader):\n"
            "    def exec_module(self, module):\n"
            "        print('boom-preseed', sys.modules.get(module.__name__) is module)\n"
            "        sys.modules[module.__name__] = 'partial'\n"
            "        raise RuntimeError('boom')\n"
            "\n"
            "class SubstituteLoader(importlib.machinery.SourceFileLoader):\n"
            "    def exec_module(self, module):\n"
            "        print('sub-preseed', sys.modules.get(module.__name__) is module)\n"
            "        sys.modules[module.__name__] = 'substitute'\n"
            "\n"
            "class ExistingBoomLoader(importlib.machinery.SourceFileLoader):\n"
            "    def __init__(self, fullname, path, previous):\n"
            "        super().__init__(fullname, path)\n"
            "        self.previous = previous\n"
            "\n"
            "    def exec_module(self, module):\n"
            "        print('existing-is-previous', module is self.previous)\n"
            "        sys.modules[module.__name__] = 'partial-existing'\n"
            "        raise RuntimeError('boom-existing')\n"
            "\n"
            "sys.modules.pop('demo_lm', None)\n"
            "show('boom', lambda: BoomLoader('demo_lm', path).load_module('demo_lm'))\n"
            "print('boom-after', 'demo_lm' in sys.modules, sys.modules.get('demo_lm'))\n"
            "sys.modules.pop('demo_lm', None)\n"
            "show('sub', lambda: SubstituteLoader('demo_lm', path).load_module('demo_lm'))\n"
            "print('sub-after', sys.modules.get('demo_lm'))\n"
            "previous = types.ModuleType('demo_lm')\n"
            "sys.modules['demo_lm'] = previous\n"
            "show('existing-boom', lambda: ExistingBoomLoader('demo_lm', path, previous).load_module('demo_lm'))\n"
            "print('existing-after', sys.modules.get('demo_lm'))\n"
            "sys.modules.pop('demo_real', None)\n"
            "module = importlib.machinery.SourceFileLoader('demo_real', path).load_module('demo_real')\n"
            "print('real', module.__name__, module.value, sys.modules.get('demo_real') is module)\n"
        ),
        "sourcefileloader_load_module_transaction",
        session_id="pytest-native-importlib-load-module-transaction",
        cache_dir=ROOT / ".molt_cache-importlib-load-module-transaction",
        backend="cranelift",
        source_relpath="main.py",
        extra_files={
            "runtime_site/loader_target.py": "value = 41\n",
        },
        extra_build_args=[
            "--capabilities",
            "fs.read",
            "--stdlib-profile",
            "full",
        ],
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "boom-preseed True",
        "boom RuntimeError boom",
        "boom-after False None",
        "sub-preseed True",
        "sub OK 'substitute'",
        "sub-after substitute",
        "existing-is-previous True",
        "existing-boom RuntimeError boom-existing",
        "existing-after partial-existing",
        "real demo_real 41 True",
    ]
