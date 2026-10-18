from __future__ import annotations

import inspect
import importlib
import ast
import hashlib
import os
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import cast

import pytest

import molt.cli as cli
from molt.cli import frontend_execution
from molt.cli import frontend_pipeline
from molt.cli import module_cache
from molt.cli.models import _SerialFrontendLoweringContext
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import _ModuleSourceCatalog, _ModuleSourceLease
from molt.frontend import SimpleTIRGenerator
from molt.target_python import TargetPythonVersion

frontend_worker = importlib.import_module("molt.cli.frontend_worker")

_FRONTEND_WORKER_NAMES = (
    "_frontend_lower_module_worker",
    "_lower_module_serial_with_context",
    "_module_frontend_generator",
    "_module_frontend_payload",
    "_phase_timeout",
    "_prepare_frontend_parallel_batch",
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
            module_source_catalog=_ModuleSourceCatalog(
                {"app": _ModuleSourceLease.path_backed(path)}
            ),
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
        message = "changed"
    else:
        target = TargetPythonVersion(3, sys.version_info.minor + 1, 0)
        message = "requires a Python"
    context = cast(
        _SerialFrontendLoweringContext,
        SimpleNamespace(
            syntax_error_modules={},
            module_source_catalog=_ModuleSourceCatalog({"app": lease}),
            generated_module_source_paths={},
            target_python=target,
        ),
    )
    with pytest.raises(frontend_worker._ModuleLowerError, match=message):
        frontend_worker._resolve_tree_for_serial_frontend_module(
            "app", path, lowering_context=context
        )


def _worker_payload(path: Path, lease: _ModuleSourceLease) -> dict[str, object]:
    return module_cache._module_worker_payload(
        "app",
        module_path=path,
        logical_source_path=str(path),
        source_lease=lease,
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        module_is_namespace=False,
        module_execution_kind="imported",
        type_facts=None,
        enable_phi=True,
        known_modules={"app"},
        known_classes_snapshot={},
        stdlib_allowlist_sorted=(),
        known_func_defaults={},
        known_func_kinds={},
        module_deps={"app": set()},
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=(),
        module_dep_closures={"app": frozenset({"app"})},
    )


@pytest.mark.parametrize("reader", ["serial", "worker", "syntax_stub"])
def test_lowering_rejects_same_size_restored_mtime_generation(
    tmp_path: Path, reader: str
) -> None:
    path = tmp_path / "app.py"
    source = "def broken(:\n" if reader == "syntax_stub" else "VALUE = 1\n"
    path.write_text(source, encoding="utf-8")
    lease = _ModuleSourceLease.path_backed(path)
    original_stat = path.stat()
    syntax_errors = {}
    if reader == "syntax_stub":
        with pytest.raises(SyntaxError) as caught:
            compile(source, str(path), "exec")
        syntax_errors["app"] = frontend_pipeline._syntax_error_info_from_exception(
            caught.value
        )
    replacement = "def breaks(:\n" if reader == "syntax_stub" else "VALUE = 2\n"
    path.write_text(replacement, encoding="utf-8")
    os.utime(path, ns=(original_stat.st_atime_ns, original_stat.st_mtime_ns))
    assert path.stat().st_size == original_stat.st_size
    assert path.stat().st_mtime_ns == original_stat.st_mtime_ns
    if reader == "worker":
        result = frontend_worker._frontend_lower_module_worker(
            _worker_payload(path, lease)
        )
        assert result["ok"] is False
        assert "changed" in result["error"]
        assert "functions" not in result
    else:
        context = cast(
            _SerialFrontendLoweringContext,
            SimpleNamespace(
                syntax_error_modules=syntax_errors,
                module_source_catalog=_ModuleSourceCatalog({"app": lease}),
                generated_module_source_paths={},
                target_python=TargetPythonVersion(3, 12, 0),
            ),
        )
        with pytest.raises(frontend_worker._ModuleLowerError, match="changed"):
            frontend_worker._resolve_tree_for_serial_frontend_module(
                "app", path, lowering_context=context
            )


@pytest.mark.parametrize("inline", [False, True])
def test_worker_success_carries_the_consumed_source_generation(
    tmp_path: Path, inline: bool
) -> None:
    path = tmp_path / "app.py"
    raw = b"# coding: latin-1\r\nVALUE = 'caf\xe9'\r\n"
    path.write_bytes(raw)
    source = "VALUE = 'different'\n"
    lease = (
        _ModuleSourceLease.inline(path, source)
        if inline
        else _ModuleSourceLease.path_backed(path)
    )
    result = frontend_worker._frontend_lower_module_worker(_worker_payload(path, lease))
    assert result["ok"] is True
    assert result["functions"]
    init_symbol = SimpleTIRGenerator.module_init_symbol("app")
    assert init_symbol in result["func_code_ids"]
    assert "molt_main" not in result["func_code_ids"]
    assert any(function["name"] == init_symbol for function in result["functions"])
    for function in result["functions"]:
        assert function["name"] != "molt_main"
        for op in function["ops"]:
            assert op["kind"] != "code_slots_init"
            if op["kind"] in {"code_slot_set", "trace_enter_slot"}:
                assert op["s_value"] in result["func_code_ids"]
                assert op.get("value") is None
    consumed_bytes = source.encode("utf-8") if inline else raw
    assert result["source_sha256"] == hashlib.sha256(consumed_bytes).hexdigest()


def test_serial_payload_uses_serialized_module_owners_without_rewriting_records() -> (
    None
):
    gen = SimpleTIRGenerator(module_name="app")
    gen.visit(ast.parse("def answer(value):\n    return value + 1\n"))
    document = gen.to_json(scope="module")
    assert "molt_main" in gen.func_code_ids
    assert "molt_main" not in document["code_slots"]
    payload = frontend_worker._module_frontend_payload(
        gen,
        document,
        source_sha256="a" * 64,
        visit_s=0.1,
        lower_s=0.2,
        total_s=0.3,
    )
    assert payload["functions"] is document["functions"]
    assert payload["func_code_ids"] == document["code_slots"]
    assert payload["func_code_ids"] != gen.func_code_ids
    for source, transferred in zip(
        document["functions"], payload["functions"], strict=True
    ):
        assert transferred is source
        assert transferred["ops"] is source["ops"]


def test_worker_refuses_a_missing_source_lease_without_throwing(tmp_path: Path) -> None:
    path = tmp_path / "app.py"
    path.write_text("VALUE = 1\n", encoding="utf-8")
    payload = _worker_payload(path, _ModuleSourceLease.path_backed(path))
    del payload["source_lease"]
    result = frontend_worker._frontend_lower_module_worker(payload)
    assert result["ok"] is False
    assert "missing source lease" in result["error"]
    assert "functions" not in result


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


def _parser_error_signature(error: SyntaxError) -> tuple[object, ...]:
    return (
        type(error).__name__,
        error.args,
        error.msg,
        error.filename,
        error.lineno,
        error.offset,
        error.text,
        error.end_lineno,
        error.end_offset,
        error.print_file_and_line,
        str(error),
    )


@pytest.mark.parametrize(
    "source",
    [
        "def broken(:\n    pass\n",
        "if True:\npass\n",
        "if True:\n\tpass\n        pass\n",
        "value = ('α',\n         ) +\n",
        b"# coding: latin-1\nvalue = 'caf\xe9' +\n",
        b"# coding: not-a-real-encoding\nvalue = 1\n",
        b"# coding: utf-8\nvalue = '\xff'\n",
    ],
)
def test_module_syntax_error_keeps_native_class_args_and_spans(source) -> None:
    import pickle
    from molt.cli.frontend_pipeline import _syntax_error_info_from_exception
    from molt.frontend.lowering.exception_lowering import MoltSyntaxError

    filename = "/logical/package/invalid_module.py"
    with pytest.raises(SyntaxError) as caught:
        ast.parse(source, filename=filename)
    original = caught.value
    info = pickle.loads(pickle.dumps(_syntax_error_info_from_exception(original)))
    assert info.exception_name == type(original).__name__
    assert info.args == original.args
    tree = frontend_worker._syntax_error_stub_ast(info)
    assert len(tree.body) == 1 and isinstance(tree.body[0], ast.Raise)
    expression = tree.body[0].exc
    assert isinstance(expression, MoltSyntaxError)
    args = tuple(ast.literal_eval(value) for value in expression.args)
    restored = type(original)(*args)
    restored.print_file_and_line = ast.literal_eval(expression.print_file_and_line)
    with pytest.raises(type(original)) as transported:
        raise restored
    assert type(transported.value) is type(original)
    assert _parser_error_signature(transported.value) == _parser_error_signature(
        original
    )
    assert not any(isinstance(node, ast.Name) for node in ast.walk(tree))

    # Exercise the actual protected constructor producer, not a Python fake.
    gen = SimpleTIRGenerator(module_name="invalid_module")
    gen.visit(tree)
    operations = [op for function in gen.funcs_map.values() for op in function["ops"]]
    constructors = [op for op in operations if op.kind == "EXCEPTION_NEW"]
    assert len(constructors) == 1
    constructor = constructors[0]
    assert any(
        op.kind == "CONST_STR"
        and op.result == constructor.args[0]
        and op.args == [type(original).__name__]
        for op in operations
    )
    assert any(op.kind == "RAISE" for op in operations)


def test_module_syntax_error_preserves_print_flag_and_missing_details() -> None:
    from molt.cli.frontend_pipeline import _syntax_error_info_from_exception

    original = SyntaxError("frontend unavailable")
    original.print_file_and_line = 1
    info = _syntax_error_info_from_exception(original)
    tree = frontend_worker._syntax_error_stub_ast(info)
    expression = tree.body[0].exc
    restored = SyntaxError(*(ast.literal_eval(arg) for arg in expression.args))
    restored.print_file_and_line = ast.literal_eval(expression.print_file_and_line)
    assert _parser_error_signature(restored) == _parser_error_signature(original)
    gen = SimpleTIRGenerator(module_name="invalid_module")
    gen.visit(tree)
    operations = [op for function in gen.funcs_map.values() for op in function["ops"]]
    assert any(
        op.kind == "SETATTR_GENERIC_OBJ" and op.args[1] == "print_file_and_line"
        for op in operations
    )


@pytest.mark.parametrize("source_kind", ["inline", "path"])
def test_serial_module_error_uses_captured_payload_without_source_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, source_kind: str
) -> None:
    from molt.cli.frontend_pipeline import _syntax_error_info_from_exception

    path = tmp_path / "invalid.py"
    source = "if True:\npass\n"
    if source_kind == "path":
        path.write_text(source, encoding="utf-8")
        lease = _ModuleSourceLease.path_backed(path)
    else:
        lease = _ModuleSourceLease.inline(path, source)
    with pytest.raises(IndentationError) as caught:
        ast.parse(source, filename="logical/invalid.py")
    info = _syntax_error_info_from_exception(caught.value)
    monkeypatch.setattr(
        _ModuleSourceCatalog,
        "read_source",
        lambda *args: pytest.fail(
            "parser payload must not be reconstructed from source"
        ),
    )
    context = cast(
        _SerialFrontendLoweringContext,
        SimpleNamespace(
            syntax_error_modules={"broken": info},
            module_source_catalog=_ModuleSourceCatalog({"broken": lease}),
        ),
    )
    tree = frontend_worker._resolve_tree_for_serial_frontend_module(
        "broken",
        path,
        lowering_context=context,
    )
    expression = tree.body[0].exc
    assert expression.exception_name == "IndentationError"
    assert tuple(ast.literal_eval(arg) for arg in expression.args) == caught.value.args
