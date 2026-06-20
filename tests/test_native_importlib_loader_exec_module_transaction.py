from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


_HELPER_PATH = Path(__file__).with_name("test_native_import_bootstrap_regressions.py")
_HELPER_SPEC = importlib.util.spec_from_file_location(
    "_molt_native_import_bootstrap_helpers_for_loader_exec_transaction",
    _HELPER_PATH,
)
assert _HELPER_SPEC is not None
assert _HELPER_SPEC.loader is not None
_HELPERS = importlib.util.module_from_spec(_HELPER_SPEC)
sys.modules[_HELPER_SPEC.name] = _HELPERS
_HELPER_SPEC.loader.exec_module(_HELPERS)

ROOT = _HELPERS.ROOT
_build_and_run_with_env = _HELPERS._build_and_run_with_env


def test_native_sourcefileloader_exec_module_transaction_preserves_body_mutations(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "runtime_site" / "loader_exec_target.py"
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import importlib.util\n"
            "import sys\n"
            f"path = {str(module_path)!r}\n"
            "spec = importlib.util.spec_from_file_location('demo_exec_tx', path)\n"
            "module = importlib.util.module_from_spec(spec)\n"
            "sys.modules.pop('demo_exec_tx', None)\n"
            "spec.loader.exec_module(module)\n"
            "print(module.value)\n"
            "print(module.__loader__ == 'mutated-loader')\n"
            "print(module.__file__ == 'mutated-file')\n"
            "print(module.__package__ == 'mutated-package')\n"
        ),
        "sourcefileloader_exec_module_transaction",
        session_id="pytest-native-importlib-loader-exec-transaction",
        cache_dir=ROOT / ".molt_cache-importlib-loader-exec-transaction",
        backend="cranelift",
        source_relpath="main.py",
        extra_files={
            "runtime_site/loader_exec_target.py": (
                "value = 505\n"
                "__loader__ = 'mutated-loader'\n"
                "__file__ = 'mutated-file'\n"
                "__package__ = 'mutated-package'\n"
            ),
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
        "505",
        "True",
        "True",
        "True",
    ]
