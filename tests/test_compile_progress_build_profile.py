from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = ROOT / "tools" / "compile_progress.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("compile_progress_under_test", MODULE_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_compile_progress_build_command_uses_build_profile_flag() -> None:
    module = _load_module()
    case = module.CaseSpec(
        name="dev_cold",
        profile="dev",
        cache_mode="cache-report",
        daemon=True,
    )

    cmd = module._build_molt_build_cmd(
        case=case,
        python_version="3.12",
        script_path="examples/hello.py",
        out_dir=Path("bench/results/out"),
        diagnostics_path=None,
    )

    assert "--build-profile" in cmd
    assert "dev" in cmd
    assert "--profile" not in cmd
