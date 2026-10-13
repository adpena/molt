from __future__ import annotations

import inspect
import importlib
import ast
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import cast

import pytest

import molt.cli as cli
from molt.cli import frontend_execution
from molt.cli.models import _SerialFrontendLoweringContext
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import _ModuleSourceCatalog, _ModuleSourceLease
from molt.frontend import SimpleTIRGenerator
from molt.target_python import TargetPythonVersion

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


def test_serial_lowering_mutation_does_not_change_discovery_tree(
    tmp_path: Path,
) -> None:
    path = tmp_path / "app.py"
    source = (
        "class Private:\n"
        "    __value = 7\n"
        "    def value(self):\n"
        "        return self.__value\n"
    )
    path.write_text(source, encoding="utf-8")
    cache = _ModuleResolutionCache()
    parsed = cache.parse_module_ast(path, source, filename=str(path))
    before = ast.dump(parsed.tree, include_attributes=True)
    context = cast(
        _SerialFrontendLoweringContext,
        SimpleNamespace(
            syntax_error_modules={},
            module_trees={},
            module_source_catalog=_ModuleSourceCatalog(
                {"app": _ModuleSourceLease.path_backed(path)}
            ),
            module_resolution_cache=cache,
            generated_module_source_paths={},
            target_python=TargetPythonVersion(3, 12, 0),
        ),
    )
    tree = frontend_worker._resolve_tree_for_serial_frontend_module(
        "app", path, lowering_context=context
    )
    gen = SimpleTIRGenerator()
    gen.visit(tree)
    assert any(
        isinstance(node, ast.Attribute) and node.attr == "_Private__value"
        for node in ast.walk(tree)
    )
    assert ast.dump(parsed.tree, include_attributes=True) == before
    assert (
        cache.parse_module_ast(path, source, filename=str(path), retain=False) is parsed
    )


@pytest.mark.parametrize("changed", ["source", "target"])
def test_serial_lowering_keeps_source_lease_and_target_refusal(
    tmp_path: Path, changed: str
) -> None:
    path = tmp_path / "app.py"
    source = "VALUE = 1\n"
    path.write_text(source, encoding="utf-8")
    cache = _ModuleResolutionCache()
    cache.parse_module_ast(path, source, filename=str(path))
    lease = _ModuleSourceLease.path_backed(path)
    target = TargetPythonVersion(3, 12, 0)
    if changed == "source":
        path.write_text("VALUE = 100\n", encoding="utf-8")
        message = "Source lease .* changed size"
    else:
        target = TargetPythonVersion(3, sys.version_info.minor + 1, 0)
        message = "requires a Python"
    context = cast(
        _SerialFrontendLoweringContext,
        SimpleNamespace(
            syntax_error_modules={},
            module_trees={},
            module_source_catalog=_ModuleSourceCatalog({"app": lease}),
            module_resolution_cache=cache,
            generated_module_source_paths={},
            target_python=target,
        ),
    )
    with pytest.raises(frontend_worker._ModuleLowerError, match=message):
        frontend_worker._resolve_tree_for_serial_frontend_module(
            "app", path, lowering_context=context
        )


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
