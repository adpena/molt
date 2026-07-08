from __future__ import annotations

import inspect
import importlib

import molt.cli as cli
from molt.cli import frontend_execution

frontend_worker = importlib.import_module("molt.cli.frontend_worker")

_FRONTEND_WORKER_NAMES = (
    "_format_syntax_error_message",
    "_frontend_lower_module_worker",
    "_lower_module_serial_with_context",
    "_module_frontend_generator",
    "_module_frontend_payload",
    "_phase_timeout",
    "_prepare_frontend_parallel_batch",
    "_read_worker_source_lease",
    "_resolve_tree_for_serial_frontend_module",
    "_run_serial_frontend_lower_with_context",
    "_syntax_error_stub_ast",
)

_FRONTEND_WORKER_DEFINITIONS = tuple(f"def {name}(" for name in _FRONTEND_WORKER_NAMES)


def test_cli_frontend_worker_authority_is_single_home() -> None:
    for name in _FRONTEND_WORKER_NAMES:
        assert hasattr(frontend_worker, name), name
        assert not hasattr(frontend_execution, name), name
        assert not hasattr(cli, name), name

    frontend_execution_source = inspect.getsource(frontend_execution)
    cli_source = inspect.getsource(cli)
    for marker in _FRONTEND_WORKER_DEFINITIONS:
        assert marker not in frontend_execution_source
        assert marker not in cli_source


def test_phase_timeout_enforced_off_main_thread() -> None:
    # Regression for the configured!=effective metabug: MOLT_FRONTEND_PHASE_TIMEOUT
    # was a SIGALRM-only no-op off the POSIX main thread — i.e. on Windows AND in
    # every parallel-lowering worker thread. The portable watchdog must now enforce
    # the bound there too, interrupting a busy Python loop within the timeout.
    import threading
    import time

    outcome: dict[str, str] = {}

    def worker() -> None:
        try:
            with frontend_worker._phase_timeout(0.2, phase_name="test-phase"):
                deadline = time.monotonic() + 5.0
                while time.monotonic() < deadline:
                    pass  # busy Python loop; the watchdog must interrupt it
            outcome["result"] = "not-enforced"
        except TimeoutError:
            outcome["result"] = "timed-out"
        except BaseException as exc:  # noqa: BLE001
            outcome["result"] = f"other:{type(exc).__name__}"

    thread = threading.Thread(target=worker)
    start = time.monotonic()
    thread.start()
    thread.join(timeout=10.0)
    elapsed = time.monotonic() - start
    assert not thread.is_alive(), "worker never finished; phase timeout not enforced"
    assert outcome.get("result") == "timed-out", outcome
    assert elapsed < 3.0, f"phase timeout fired too late ({elapsed:.1f}s)"
