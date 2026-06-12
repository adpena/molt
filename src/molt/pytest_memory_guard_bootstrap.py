from __future__ import annotations

from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.pytest_memory_guard_bootstrap import (  # noqa: E402,F401
    ensure_current_file_test_script_memory_guard,
    ensure_python_test_memory_guard,
    ensure_pytest_memory_guard,
    ensure_repo_test_module_memory_guard,
    ensure_repo_test_script_memory_guard,
    pytest_load_initial_conftests,
    pytest_runtest_call,
    pytest_runtest_logfinish,
    pytest_runtest_logstart,
    pytest_runtest_setup,
    pytest_runtest_teardown,
)
