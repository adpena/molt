"""The CLI never uses the process environment as a parameter channel (HF-60).

A value the CLI resolves (``--backend``, ``--portable``, ``--wasm-profile``,
``--type-gate``, ``--stdlib-profile``, the Luau chunk size, the runtime policy
flags) travels as a typed parameter; a child process receives its share in an
environment mapping built for that child. Before HF-60 the CLI wrote these to
``os.environ``: ``MOLT_BACKEND`` and ``MOLT_MODULE_CHUNK_OPS`` stayed there for
the rest of the process, so a later in-process build (library use, the batch
build server, a test) silently inherited the previous choice.

The behavioral tests below fail on that code: each observes ``os.environ``
during and after the operation, or the value a later operation receives. The
structural test keeps the family closed for every module under ``src/molt``.
"""

from __future__ import annotations

import ast
import io
import json
import os
import sys
from pathlib import Path
from typing import Any

import pytest

import molt.cli as cli
from molt.backend_environment import CodegenSelection
from molt.cli import (
    backend_execution,
    build_inputs,
    build_output_layout,
    entrypoint_dispatch,
    entrypoint_parser,
    quality_commands,
)
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.cli.config_resolution import _select_codegen_backend
from molt.cli.project_roots import _find_project_root

ROOT = Path(__file__).resolve().parents[2]
_SELECTION_NAMES = (
    "MOLT_AUDIT_ENABLED",
    "MOLT_AUDIT_OUTPUT",
    "MOLT_AUDIT_SINK",
    "MOLT_BACKEND",
    "MOLT_CAPABILITY_TIER",
    "MOLT_IO_MODE",
    "MOLT_MODULE_CHUNK_OPS",
    "MOLT_PORTABLE",
    "MOLT_STDLIB_PROFILE",
    "MOLT_TYPE_GATE",
    "MOLT_WASM_PROFILE",
)


@pytest.fixture
def clean_selection_env(monkeypatch: pytest.MonkeyPatch) -> None:
    for name in _SELECTION_NAMES:
        monkeypatch.delenv(name, raising=False)


@pytest.mark.parametrize(
    ("target", "choice", "expected"),
    [
        ("llvm", "auto", ("native", "llvm", None)),
        ("native", "llvm", ("native", "llvm", None)),
        ("native", "auto", ("native", "cranelift", None)),
        ("wasm", "cranelift", ("wasm", "cranelift", None)),
    ],
)
def test_backend_selection_returns_the_backend_and_writes_nothing(
    clean_selection_env: None,
    target: str,
    choice: str,
    expected: tuple[str, str, None],
) -> None:
    before = dict(os.environ)

    assert _select_codegen_backend(target, choice) == expected
    assert dict(os.environ) == before


def test_backend_selection_reports_conflicts_without_selecting() -> None:
    target, backend, error = _select_codegen_backend("llvm", "cranelift")

    assert (target, backend) == ("llvm", "cranelift")
    assert error is not None and "--target native --backend llvm" in error


def _dispatch_build(argv: list[str], build_fn: Any) -> int:
    args = entrypoint_parser._build_entrypoint_parser().parse_args(argv)
    return entrypoint_dispatch._dispatch_entrypoint_command(
        args,
        build_fn=build_fn,
        config_root=_find_project_root(Path.cwd()),
        config={},
        build_cfg={},
        run_cfg={},
        compare_cfg={},
        test_cfg={},
        diff_cfg={},
        extension_cfg={},
        publish_cfg={},
        cfg_capabilities=None,
    )


def test_a_later_build_cannot_observe_the_previous_backend(
    clean_selection_env: None, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    seen: list[tuple[str, str | None]] = []

    def build_fn(*_args: Any, **kwargs: Any) -> int:
        seen.append((kwargs["codegen_backend"], os.environ.get("MOLT_BACKEND")))
        return 0

    assert _dispatch_build(["build", "app.py", "--target", "llvm"], build_fn) == 0
    assert "MOLT_BACKEND" not in os.environ
    assert _dispatch_build(["build", "app.py"], build_fn) == 0

    assert seen == [("llvm", None), ("cranelift", None)]


def test_build_passes_every_selection_explicitly(
    clean_selection_env: None, monkeypatch: pytest.MonkeyPatch
) -> None:
    before = dict(os.environ)
    captured: dict[str, Any] = {}

    def fake_prepare_inputs(**kwargs: Any) -> tuple[None, int]:
        captured.update(kwargs)
        captured["environ"] = dict(os.environ)
        return None, 9  # stop before any source is read

    monkeypatch.setattr(build_inputs, "_prepare_build_inputs", fake_prepare_inputs)

    rc = cli.build(
        "app.py",
        target="wasm",
        trusted=True,
        portable=True,
        wasm_profile="pure",
        type_gate=True,
        audit_log="jsonl:stderr",
        io_mode="virtual",
        stdlib_profile="full",
        codegen_backend="llvm",
    )

    assert rc == 9
    # Nothing reached the process environment while the build ran, or after.
    assert captured["environ"] == before
    assert dict(os.environ) == before
    assert captured["codegen"] == CodegenSelection(
        backend="llvm", portable=True, wasm_profile="pure", type_gate=True
    )
    assert (captured["trusted"], captured["audit_log"], captured["io_mode"]) == (
        True,
        "jsonl:stderr",
        "virtual",
    )


def test_runtime_policy_flags_reach_the_policy_as_one_mapping(
    clean_selection_env: None,
) -> None:
    before = dict(os.environ)

    env = build_inputs._runtime_policy_flag_environment(
        {"KEEP": "1"}, trusted=True, audit_log="jsonl:stderr", io_mode="virtual"
    )

    assert env["KEEP"] == "1"
    assert env["MOLT_CAPABILITY_TIER"] == "full"
    assert (env["MOLT_AUDIT_SINK"], env["MOLT_IO_MODE"]) == ("jsonl", "virtual")
    assert dict(os.environ) == before


def test_codegen_selection_projects_into_a_copy() -> None:
    base = {"MOLT_PORTABLE": "0", "MOLT_WASM_PROFILE": "full", "OTHER": "x"}

    default = CodegenSelection().environment(base)
    chosen = CodegenSelection(
        backend="llvm", portable=True, wasm_profile="pure", type_gate=True
    ).environment(base)

    assert base == {"MOLT_PORTABLE": "0", "MOLT_WASM_PROFILE": "full", "OTHER": "x"}
    # Unset optional fields keep the caller's value: MOLT_PORTABLE=0 still
    # opts in to host-CPU code.
    assert default == {**base, "MOLT_BACKEND": "cranelift"}
    assert chosen == {
        "MOLT_BACKEND": "llvm",
        "MOLT_PORTABLE": "1",
        "MOLT_WASM_PROFILE": "pure",
        "MOLT_TYPE_GATE": "1",
        "OTHER": "x",
    }
    with pytest.raises(ValueError, match="unknown codegen backend"):
        CodegenSelection(backend="gcc")  # type: ignore[arg-type]


def test_daemon_requests_carry_the_selection_not_the_process_environment(
    clean_selection_env: None,
) -> None:
    payload, error = backend_execution._backend_daemon_compile_request_bytes(
        ir={"functions": []},
        backend_output=Path("out.wasm"),
        artifact_contract=resolve_backend_artifact_contract(
            target="wasm", emit_mode="wasm"
        ),
        wasm_link=False,
        wasm_data_base=None,
        wasm_table_base=None,
        cache_key="key",
        function_cache_key="function-key",
        config_digest="config",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        request_environment=CodegenSelection(
            backend="llvm", wasm_profile="pure"
        ).environment(os.environ),
    )

    assert error is None and payload is not None
    env = json.loads(payload)["env"]
    # A daemon resets these from its catalog per request: the request must
    # carry them, or a warm daemon falls back to Cranelift.
    assert (env["MOLT_BACKEND"], env["MOLT_WASM_PROFILE"]) == ("llvm", "pure")
    assert "MOLT_BACKEND" not in os.environ


def test_luau_layout_leaves_the_chunk_size_to_the_frontend(
    clean_selection_env: None, tmp_path: Path
) -> None:
    layout = build_output_layout._resolve_build_output_layout(
        target="luau",
        trusted=False,
        require_linked=False,
        linked=False,
        linked_output=None,
        emit=None,
        output=None,
        emit_ir=None,
        artifacts_root=tmp_path / "artifacts",
        bin_root=tmp_path / "bin",
        output_root=tmp_path / "dist",
        output_base="app",
        out_dir_path=None,
        project_root=tmp_path,
    )

    assert layout.is_luau_transpile
    assert "MOLT_MODULE_CHUNK_OPS" not in os.environ


def test_batch_server_selects_each_request_backend_explicitly(
    clean_selection_env: None,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    seen: list[dict[str, Any]] = []

    def build_fn(**kwargs: Any) -> int:
        seen.append(
            {
                "target": kwargs["target"],
                "backend": kwargs["codegen_backend"],
                "stdlib_profile": kwargs["stdlib_profile"],
                "environ_backend": os.environ.get("MOLT_BACKEND"),
                "environ_profile": os.environ.get("MOLT_STDLIB_PROFILE"),
            }
        )
        return 0

    requests = [
        {
            "id": 1,
            "op": "build",
            "params": {"target": "llvm", "stdlib_profile": "full"},
        },
        {"id": 2, "op": "build", "params": {"backend": "llvm"}},
        {"id": 3, "op": "build", "params": {}},
        {"id": 4, "op": "build", "params": {"target": "llvm", "backend": "cranelift"}},
        {"id": 5, "op": "shutdown"},
    ]
    monkeypatch.setattr(
        sys, "stdin", io.StringIO("".join(json.dumps(r) + "\n" for r in requests))
    )

    assert quality_commands._internal_batch_build_server(build_fn=build_fn) == 0

    assert seen == [
        {
            "target": "native",
            "backend": "llvm",
            "stdlib_profile": "full",
            "environ_backend": None,
            "environ_profile": None,
        },
        {
            "target": "native",
            "backend": "llvm",
            "stdlib_profile": "auto",
            "environ_backend": None,
            "environ_profile": None,
        },
        {
            "target": "native",
            "backend": "cranelift",
            "stdlib_profile": "auto",
            "environ_backend": None,
            "environ_profile": None,
        },
    ]
    responses = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    assert [r["ok"] for r in responses] == [True, True, True, False, True]
    assert "--target native --backend llvm" in responses[3]["error"]


# --- structure: no other process-environment writer under src/molt ---------

# The one sanctioned overlay (see its docstring) and the pytest bootstrap,
# which configures its own test process (pytest reads PYTEST_DEBUG_TEMPROOT and
# its workers inherit MOLT_PYTEST_CURRENT_TEST_FILE from the process).
_SANCTIONED_WRITERS = {
    "src/molt/cli/env_overrides.py",
    "src/molt/pytest_memory_guard_bootstrap.py",
}
# Open under HF-60: the native-arch perf policy appends to RUSTFLAGS in the
# process environment, which every later Cargo build and compiler fingerprint
# reads. Moving it needs an explicit flags parameter through every Cargo
# environment authority, which only Cargo builds can prove.
_OPEN_WRITERS = {("src/molt/cli/build_inputs.py", "_append_rustflags")}
_MUTATORS = {"update", "setdefault", "pop", "popitem", "clear"}
_MUTABLE_ANNOTATIONS = ("MutableMapping", "dict", "Dict")


def _is_os_environ(node: ast.AST) -> bool:
    return (
        isinstance(node, ast.Attribute)
        and node.attr == "environ"
        and isinstance(node.value, ast.Name)
        and node.value.id == "os"
    )


def _mutable_parameters(trees: dict[str, ast.Module]) -> dict[str, set[str | int]]:
    """Function name -> parameter names/positions annotated as a mutable map."""
    found: dict[str, set[str | int]] = {}
    for tree in trees.values():
        for node in ast.walk(tree):
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            params = [*node.args.posonlyargs, *node.args.args, *node.args.kwonlyargs]
            positional = [*node.args.posonlyargs, *node.args.args]
            for param in params:
                if param.annotation is None:
                    continue
                text = ast.unparse(param.annotation)
                if text.startswith(_MUTABLE_ANNOTATIONS):
                    entry = found.setdefault(node.name, set())
                    entry.add(param.arg)
                    if param in positional:
                        entry.add(positional.index(param))
    return found


def _scope_nodes(body: list[ast.stmt]) -> list[ast.AST]:
    """Every node of one scope, without descending into nested functions."""
    nodes: list[ast.AST] = []
    stack: list[ast.AST] = list(body)
    while stack:
        node = stack.pop()
        nodes.append(node)
        for child in ast.iter_child_nodes(node):
            if not isinstance(
                child, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)
            ):
                stack.append(child)
    return nodes


def _environment_writes(path: str, tree: ast.Module, mutable: dict) -> list[str]:
    writes: list[str] = []

    def note(node: ast.AST, what: str) -> None:
        writes.append(f"{path}:{getattr(node, 'lineno', 0)}: {what}")

    scopes = [tree.body] + [
        node.body
        for node in ast.walk(tree)
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    ]
    for body in scopes:
        nodes = _scope_nodes(body)
        aliases = {
            target.id
            for node in nodes
            if isinstance(node, ast.Assign) and _is_os_environ(node.value)
            for target in node.targets
            if isinstance(target, ast.Name)
        }

        def is_env(node: ast.AST) -> bool:
            return _is_os_environ(node) or (
                isinstance(node, ast.Name) and node.id in aliases
            )

        for node in nodes:
            targets: list[ast.AST] = []
            if isinstance(node, (ast.Assign, ast.Delete)):
                targets = list(node.targets)
            elif isinstance(node, (ast.AugAssign, ast.AnnAssign)):
                targets = [node.target]
            for target in targets:
                if isinstance(target, ast.Subscript) and is_env(target.value):
                    note(node, "assigns or deletes an environment item")
            if not isinstance(node, ast.Call):
                continue
            callee = node.func
            if (
                isinstance(callee, ast.Attribute)
                and callee.attr in _MUTATORS
                and is_env(callee.value)
            ):
                note(node, f"calls environ.{callee.attr}")
            if (
                isinstance(callee, ast.Attribute)
                and callee.attr in {"putenv", "unsetenv"}
                and isinstance(callee.value, ast.Name)
                and callee.value.id == "os"
            ):
                note(node, f"calls os.{callee.attr}")
            name = (
                callee.attr
                if isinstance(callee, ast.Attribute)
                else callee.id
                if isinstance(callee, ast.Name)
                else None
            )
            if name is None or name not in mutable or (path, name) in _OPEN_WRITERS:
                continue
            passed = [
                *(arg for index, arg in enumerate(node.args) if index in mutable[name]),
                *(k.value for k in node.keywords if k.arg in mutable[name]),
            ]
            if any(is_env(arg) for arg in passed):
                note(node, f"passes os.environ to a mutable parameter of {name}")
    return sorted(set(writes))


def _source_trees() -> dict[str, ast.Module]:
    trees: dict[str, ast.Module] = {}
    for path in sorted((ROOT / "src" / "molt").rglob("*.py")):
        relative = path.relative_to(ROOT).as_posix()
        if relative.startswith("src/molt/stdlib/"):
            continue  # compiled guest code: os.environ there is the program's
        trees[relative] = ast.parse(path.read_text(encoding="utf-8"))
    return trees


def test_no_cli_module_writes_the_process_environment() -> None:
    trees = _source_trees()
    mutable = _mutable_parameters(trees)
    violations = [
        write
        for path, tree in trees.items()
        if path not in _SANCTIONED_WRITERS
        for write in _environment_writes(path, tree, mutable)
    ]

    assert violations == []


def test_the_structure_check_finds_each_write_shape() -> None:
    """The scan is the oracle above, so prove it sees every shape it claims."""
    source = """
import os
from collections.abc import MutableMapping


def _fill(env: MutableMapping[str, str]) -> None:
    env["X"] = "1"


def writer() -> None:
    os.environ["A"] = "1"
    os.environ.update({"B": "1"})
    os.environ.setdefault("C", "1")
    del os.environ["D"]
    os.putenv("E", "1")
    alias = os.environ
    alias["F"] = "1"
    _fill(os.environ)


def reader() -> str:
    copy = dict(os.environ)
    copy["G"] = "1"
    return os.environ.get("H", "")
"""
    tree = ast.parse(source)
    writes = _environment_writes("fixture.py", tree, _mutable_parameters({"f": tree}))
    found = sorted((int(line.split(":")[1]), line.split(": ", 1)[1]) for line in writes)

    # Line 7 writes a parameter, not os.environ: the call site that passes
    # os.environ to it (line 18) is the violation.
    assert found == [
        (11, "assigns or deletes an environment item"),
        (12, "calls environ.update"),
        (13, "calls environ.setdefault"),
        (14, "assigns or deletes an environment item"),
        (15, "calls os.putenv"),
        (17, "assigns or deletes an environment item"),
        (18, "passes os.environ to a mutable parameter of _fill"),
    ]
