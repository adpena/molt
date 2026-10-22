"""The CLI never uses the process environment as a parameter channel (HF-60).

A value the CLI resolves (``--backend``, ``--portable``, ``--wasm-profile``,
``--stdlib-profile``, the Luau chunk size, the runtime policy
flags, the native-arch Rust CPU policy) travels as a typed parameter; a child
process receives its share in an environment mapping built for that child.
Before HF-60 the CLI wrote these to ``os.environ``: ``MOLT_BACKEND``,
``MOLT_MODULE_CHUNK_OPS`` and ``-C target-cpu=native`` in ``RUSTFLAGS`` stayed
there for the rest of the process, so a later in-process build (library use,
the batch build server, a test, a WASM build) silently inherited the previous
choice.

The behavioral tests below fail on that code: each observes ``os.environ``
during and after the operation, or the value a later operation receives. The
structural test keeps the family closed for every module under ``src/molt``.
"""

from __future__ import annotations

from molt.compilation_target import admit_target, target_name

import ast
import base64
import importlib.util
import io
import json
import os
import symtable
import sys
import unicodedata
from collections import defaultdict, deque
from collections.abc import Callable, Iterable, Iterator, Mapping
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Any

import pytest

from molt.backend_environment import CodegenSelection
from molt.cli import (
    backend_execution,
    build_inputs,
    build_output_layout,
    entrypoint_dispatch,
    entrypoint_parser,
    quality_commands,
)
from molt.cli.build_request import BuildRequest, build_request_to_payload
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.cli.config_resolution import _select_codegen_backend
from molt.cli.project_roots import _find_project_root
from molt.cli.rust_cpu_policy import (
    NATIVE_CPU_RUSTFLAGS,
    RustCpuPolicy,
    resolve_rust_cpu_policy,
)
from molt.environment_registry import load_registry
from molt.cli import build_command as cli_build_command
from tests.process_guard_common import guard_custody_env, run_guarded_test_process

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

    selected, backend, error = _select_codegen_backend(target, choice)
    assert (target_name(selected), backend, error) == expected
    assert dict(os.environ) == before


def test_backend_selection_reports_conflicts_without_selecting() -> None:
    target, backend, error = _select_codegen_backend("llvm", "cranelift")

    assert backend == "cranelift"
    assert error is not None and "--target native --backend llvm" in error


def _dispatch_build(argv: list[str], build_fn: Any) -> int:
    parser = entrypoint_parser._build_entrypoint_parser()
    args = parser.parse_args(argv)
    return entrypoint_dispatch._dispatch_entrypoint_command(
        args,
        parser=parser,
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

    def build_fn(request: BuildRequest) -> int:
        seen.append((request.codegen_backend, os.environ.get("MOLT_BACKEND")))
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

    rc = cli_build_command.build(
        BuildRequest(
            file_path="app.py",
            target=admit_target("wasm"),
            trusted=True,
            portable=True,
            wasm_profile="pure",
            audit_log="jsonl:stderr",
            io_mode="virtual",
            stdlib_profile="full",
            codegen_backend="llvm",
        )
    )

    assert rc == 9
    # Nothing reached the process environment while the build ran, or after.
    assert captured["environ"] == before
    assert dict(os.environ) == before
    assert captured["codegen"] == CodegenSelection(
        backend="llvm", portable=True, wasm_profile="pure"
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
        backend="llvm", portable=True, wasm_profile="pure"
    ).environment(base)

    assert base == {"MOLT_PORTABLE": "0", "MOLT_WASM_PROFILE": "full", "OTHER": "x"}
    # Unset optional fields keep the caller's value: MOLT_PORTABLE=0 still
    # opts in to host-CPU code. The backend's debug directory and its cache
    # root (HF-443) are always named, and the root is absolute.
    debug_dir = default["MOLT_DEBUG_ARTIFACT_DIR"]
    cache_root = default["MOLT_CACHE"]
    assert Path(cache_root).is_absolute()
    assert default == {
        **base,
        "MOLT_BACKEND": "cranelift",
        "MOLT_CACHE": cache_root,
        "MOLT_DEBUG_ARTIFACT_DIR": debug_dir,
    }
    assert chosen == {
        "MOLT_BACKEND": "llvm",
        "MOLT_CACHE": cache_root,
        "MOLT_DEBUG_ARTIFACT_DIR": debug_dir,
        "MOLT_PORTABLE": "1",
        "MOLT_WASM_PROFILE": "pure",
        "OTHER": "x",
    }
    with pytest.raises(ValueError, match="unknown codegen backend"):
        CodegenSelection(backend="gcc")  # type: ignore[arg-type]


def test_daemon_requests_carry_the_selection_not_the_process_environment(
    clean_selection_env: None,
) -> None:
    payload, error = backend_execution._backend_daemon_compile_request_bytes(
        ir_path=Path("backend-ir.msgpack"),
        backend_output=Path("out.wasm"),
        artifact_contract=resolve_backend_artifact_contract(
            target=admit_target("wasm"), emit_mode="wasm"
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
    # The daemon writes debug artifacts where the CLI says (HF-111).
    assert Path(env["MOLT_DEBUG_ARTIFACT_DIR"]).is_absolute()
    assert "MOLT_EXT_ROOT" not in env


def test_luau_layout_leaves_the_chunk_size_to_the_frontend(
    clean_selection_env: None, tmp_path: Path
) -> None:
    layout = build_output_layout._resolve_build_output_layout(
        target=admit_target("luau"),
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


@pytest.mark.parametrize("policy", ["error", "bridge", None, False])
def test_batch_server_refuses_retired_fallback_before_environment_or_build(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    policy: object,
) -> None:
    requests = [
        {
            "id": 17,
            "op": "build",
            "params": {"file_path": "app.py", "fallback": policy},
            "env_overrides": {"MOLT_RETIRED_POLICY_PROBE": "must-not-apply"},
        },
        {"id": 18, "op": "shutdown"},
    ]
    before = dict(os.environ)
    monkeypatch.setattr(
        sys, "stdin", io.StringIO("".join(json.dumps(row) + "\n" for row in requests))
    )

    def forbidden(*_args: object, **_kwargs: object):
        pytest.fail("retired RPC parameter reached environment or build selection")

    monkeypatch.setattr(quality_commands, "_temporary_env_overrides", forbidden)
    assert quality_commands._internal_batch_build_server(build_fn=forbidden) == 0
    responses = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    assert responses == [
        {
            "id": 17,
            "ok": False,
            "error": "BuildRequest: unknown fields: fallback",
        },
        {"id": 18, "ok": True, "shutdown": True},
    ]
    assert dict(os.environ) == before


def test_batch_server_selects_each_request_backend_explicitly(
    clean_selection_env: None,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    seen: list[dict[str, Any]] = []

    def build_fn(request: BuildRequest) -> int:
        seen.append(
            {
                "target": target_name(request.target),
                "backend": request.codegen_backend,
                "stdlib_profile": request.stdlib_profile,
                "environ_backend": os.environ.get("MOLT_BACKEND"),
                "environ_profile": os.environ.get("MOLT_STDLIB_PROFILE"),
            }
        )
        return 0

    requests = [
        {
            "id": 1,
            "op": "build",
            "params": build_request_to_payload(
                BuildRequest(
                    file_path="app.py", codegen_backend="llvm", stdlib_profile="full"
                )
            ),
        },
        {
            "id": 2,
            "op": "build",
            "params": {"file_path": "app.py", "codegen_backend": "llvm"},
        },
        {"id": 3, "op": "build", "params": {"file_path": "app.py"}},
        {"id": 4, "op": "build", "params": {"file_path": "app.py", "backend": "llvm"}},
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
            "stdlib_profile": None,
            "environ_backend": None,
            "environ_profile": None,
        },
        {
            "target": "native",
            "backend": "cranelift",
            "stdlib_profile": None,
            "environ_backend": None,
            "environ_profile": None,
        },
    ]
    responses = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    assert [r["ok"] for r in responses] == [True, True, True, False, True]
    assert responses[3]["error"] == "BuildRequest: unknown fields: backend"


# --- the native-arch Rust CPU policy (MOLT_PERF_PROFILE) --------------------

_RUST_FLAG_NAMES = ("RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS")


def _prepare_preamble(target: str) -> Any:
    preamble, error = build_inputs._prepare_build_preamble(
        diagnostics=False,
        diagnostics_file=None,
        diagnostics_verbosity=None,
        json_output=True,
        target=admit_target(target),
    )
    assert error is None and preamble is not None
    return preamble


def test_a_native_arch_build_leaves_no_rust_flags_for_a_later_wasm_build(
    clean_selection_env: None, monkeypatch: pytest.MonkeyPatch
) -> None:
    for name in _RUST_FLAG_NAMES:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("MOLT_PERF_PROFILE", "native-arch")
    before = dict(os.environ)

    native = _prepare_preamble("native")
    assert dict(os.environ) == before
    wasm = _prepare_preamble("wasm")

    # Before HF-60 the native build appended -C target-cpu=native to
    # os.environ["RUSTFLAGS"], and every later Cargo plan read it.
    assert dict(os.environ) == before
    assert (native.rust_cpu, native.warnings) == (RustCpuPolicy.NATIVE, [])
    assert wasm.rust_cpu is RustCpuPolicy.BASELINE
    assert len(wasm.warnings) == 1 and "'wasm'" in wasm.warnings[0]


@pytest.mark.parametrize("target", ["native", "wasm", "luau", "aarch64-apple-darwin"])
def test_an_unset_perf_profile_selects_the_baseline_cpu(target: str) -> None:
    assert resolve_rust_cpu_policy(admit_target(target), {}) == (
        RustCpuPolicy.BASELINE,
        None,
    )
    assert resolve_rust_cpu_policy(
        admit_target(target), {"MOLT_PERF_PROFILE": " "}
    ) == (
        RustCpuPolicy.BASELINE,
        None,
    )


def test_only_the_registered_perf_profile_value_is_accepted() -> None:
    # The registry is the knob's declared contract; the resolver must accept
    # exactly its values and reject every former alias and near miss.
    variable = load_registry().lookup_variable("MOLT_PERF_PROFILE")
    assert variable is not None and variable.values == ("native-arch",)
    for value in variable.values:
        policy, warning = resolve_rust_cpu_policy(
            admit_target("native"), {"MOLT_PERF_PROFILE": value}
        )
        assert (policy, warning) == (RustCpuPolicy.NATIVE, None)
    for value in ("native", "native_arch", "NATIVE-ARCH", "1", "portable"):
        with pytest.raises(ValueError, match="MOLT_PERF_PROFILE"):
            resolve_rust_cpu_policy(
                admit_target("native"), {"MOLT_PERF_PROFILE": value}
            )


def test_one_registered_knob_selects_the_native_cpu() -> None:
    # One policy, one knob: a second spelling of -C target-cpu=native was
    # retired with MOLT_PERF_PROFILE as its replacement, and no other
    # registered variable may describe that flag again.
    registry = load_registry()
    knobs = [row.name for row in registry.variables if "target-cpu" in row.summary]

    assert knobs == ["MOLT_PERF_PROFILE"]
    assert "2026-10-13" in {
        row.retired
        for row in registry.retired
        if row.replacement == "MOLT_PERF_PROFILE"
    }


def test_an_invalid_perf_profile_fails_the_build_preamble(
    clean_selection_env: None,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setenv("MOLT_PERF_PROFILE", "native")

    preamble, error = build_inputs._prepare_build_preamble(
        diagnostics=False,
        diagnostics_file=None,
        diagnostics_verbosity=None,
        json_output=True,
        target=admit_target("native"),
    )

    assert preamble is None and error is not None
    assert "MOLT_PERF_PROFILE" in capsys.readouterr().out


def test_the_native_policy_appends_the_flag_last_and_once() -> None:
    selected = ("--cfg", "configured", "-C", "target-cpu=x86-64-v3")

    native = RustCpuPolicy.NATIVE.rustflags(selected)

    # rustc applies the last -C target-cpu, so the native flag must follow
    # every flag Cargo selected.
    assert native == (*selected, "-C", "target-cpu=native")
    assert NATIVE_CPU_RUSTFLAGS == ("-C", "target-cpu=native")
    assert RustCpuPolicy.NATIVE.rustflags(native) == native
    assert RustCpuPolicy.NATIVE.rustflags(()) == ("-C", "target-cpu=native")
    assert RustCpuPolicy.BASELINE.rustflags(selected) == selected


# --- structure: no other process-environment writer under src/molt ---------

# The one sanctioned overlay (see its docstring) and the pytest bootstrap,
# which configures its own test process (pytest reads PYTEST_DEBUG_TEMPROOT and
# its workers inherit MOLT_PYTEST_CURRENT_TEST_FILE from the process).
_SANCTIONED_WRITERS = {
    "src/molt/cli/env_overrides.py",
    "src/molt/pytest_memory_guard_bootstrap.py",
}


class _Value(Enum):
    """What a name, attribute or expression may hold that writes the environment."""

    OS = "the os module"
    PRIMITIVE = "the posix or nt module, which os wraps"
    ENVIRON = "os.environ or os.environb"
    WRITER = "a function that writes the process environment"
    # What a name in a scope not paired with CPython's tables may hold, and
    # its mutators: a report they cause says it rests on that assumption.
    ASSUMED_ENVIRON = "os.environ, assumed"
    ASSUMED_WRITER = "a mutator of an assumed os.environ"


_WRITERS = {"putenv": _Value.WRITER, "unsetenv": _Value.WRITER}
# What each module's attributes hold, also as names `from <module> import` binds.
# posix.environ is a copy: writing it leaves the process environment alone.
_ATTRIBUTES = {
    _Value.OS: {"environ": _Value.ENVIRON, "environb": _Value.ENVIRON, **_WRITERS},
    _Value.PRIMITIVE: _WRITERS,
}
_MODULES = {"os": _Value.OS, "posix": _Value.PRIMITIVE, "nt": _Value.PRIMITIVE}
# Every value the scan follows starts at one of these identifiers.
_ORIGINS = frozenset(name for names in _ATTRIBUTES.values() for name in names)
# The methods of os.environ that write the process environment.
_MUTATORS = frozenset(
    {
        "update",
        "setdefault",
        "pop",
        "popitem",
        "clear",
        "__setitem__",
        "__delitem__",
        "__ior__",
    }
)
_MUTABLE_MAPS = frozenset({"MutableMapping", "dict", "Dict"})
_SCOPES = (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda, ast.ClassDef)
_COMPREHENSIONS = (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)
_COMPREHENSION_TABLES = frozenset({"listcomp", "setcomp", "dictcomp", "genexpr"})
_ScopeKey = tuple[str, int, frozenset[str]]
_Function = ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda
_Comprehension = ast.ListComp | ast.SetComp | ast.DictComp | ast.GeneratorExp


def _mangle(private: str | None, name: str) -> str:
    """CPython's ``_Py_Mangle``: in the body of class ``private``, and in every
    scope nested in it, ``__x`` means ``_private__x`` (leading underscores of
    the class name stripped), unless it ends with ``__`` or the class name is
    all underscores."""
    if private is None or not name.startswith("__") or name.endswith("__"):
        return name
    stripped = private.lstrip("_")
    return f"_{stripped}{name}" if stripped else name


@dataclass(eq=False)
class _Scope:
    """One namespace of a module.

    ``table`` is CPython's symbol table for a module, function or class body.
    A comprehension has none of its own: CPython 3.12 inlines list, set and
    dict comprehensions into the enclosing table (PEP 709), which then mixes
    a comprehension's ``for`` names with the enclosing scope's names. So a
    comprehension is a scope here that binds exactly ``targets``. An
    ``unknown`` scope is one the scan could not pair with a table, or a
    comprehension inside one: it may bind any name, and every name read in it
    may hold ``os.environ``.
    """

    table: symtable.SymbolTable | None
    parent: _Scope | None
    kind: str  # "module", "function", "class" or "comprehension"
    private: str | None = None  # the nearest enclosing class: it mangles names
    targets: frozenset[str] = frozenset()
    unknown: bool = False

    def binds(self, name: str) -> bool:
        """Whether a use in a nested function can read this scope's ``name``."""
        if self.kind == "class":
            return False
        if self.unknown:
            return True
        if self.table is None:
            return name in self.targets
        return (
            name in self.table.get_identifiers() and self.table.lookup(name).is_local()
        )

    def home(self, name: str) -> _Scope:
        """The scope whose binding of ``name`` (mangled) a use of it here reads."""
        if self.parent is None or self.unknown:
            return self
        if self.table is None:
            if name in self.targets:
                return self
            # A comprehension body is a function: it does not see a class body.
            if self.parent.kind == "class":
                return self.parent.enclosing(name)
            return self.parent.home(name)
        if name not in self.table.get_identifiers():
            # Only a scope CPython adds lists it (a type parameter's bound or
            # default, a generic function's annotations, a 3.14 annotation
            # scope), so it is global: CPython lists a closure variable in
            # every scope it passes through.
            return self.module()
        symbol = self.table.lookup(name)
        if symbol.is_local():
            return self
        if symbol.is_free():  # a closure variable or a `nonlocal` name
            return self.enclosing(name)
        return self.module()  # an implicit or declared global

    def enclosing(self, name: str) -> _Scope:
        """The nearest enclosing function or comprehension that binds ``name``,
        past every class body, else the module."""
        scope = self.parent or self
        while scope.parent is not None and not scope.binds(name):
            scope = scope.parent
        return scope

    def module(self) -> _Scope:
        scope = self
        while scope.parent is not None:
            scope = scope.parent
        return scope


def _nested_tables(table: symtable.SymbolTable) -> Iterator[symtable.SymbolTable]:
    """The function and class tables in ``table``, in CPython's creation order,
    through the tables CPython adds for comprehensions, annotations and type
    parameters."""
    for child in table.get_children():
        name = child.get_name().strip("<>")
        if child.get_type() in ("function", "class") and (
            name not in _COMPREHENSION_TABLES
        ):
            yield child
        else:
            yield from _nested_tables(child)


def _scope_key(name: str, line: int, parameters: Iterable[str]) -> _ScopeKey:
    return name.strip("<>"), line, frozenset(parameters)


def _named(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    return None


def _is_mutable_map(annotation: ast.AST) -> bool:
    """``MutableMapping``, ``dict`` or ``Dict``: bare, qualified, generic, in a
    string, or one member of ``Optional``, ``Union`` or ``|`` (one explicit
    stack: a union can be thousands of members long)."""
    stack = [annotation]
    while stack:
        node = stack.pop()
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            try:
                stack.append(ast.parse(node.value, mode="eval").body)
            except (SyntaxError, ValueError, MemoryError, RecursionError):
                pass  # not an annotation CPython could evaluate
            continue
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.BitOr):
            stack += [node.left, node.right]
            continue
        if isinstance(node, ast.Subscript):
            if _named(node.value) in ("Optional", "Union"):
                members = node.slice
                stack += members.elts if isinstance(members, ast.Tuple) else [members]
                continue
            node = node.value
        if _named(node) in _MUTABLE_MAPS:
            return True
    return False


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
                if param.annotation is None or not _is_mutable_map(param.annotation):
                    continue
                entry = found.setdefault(node.name, set())
                entry.add(param.arg)
                if param in positional:
                    entry.add(positional.index(param))
    return found


def _captures(pattern: ast.pattern) -> Iterator[str]:
    """The names a case pattern binds to the whole subject: a capture, an
    ``as`` name and the captures inside it, through ``|``."""
    stack = [pattern]
    while stack:
        node = stack.pop()
        if isinstance(node, ast.MatchAs):
            if node.name is not None:
                yield node.name
            if node.pattern is not None:
                stack.append(node.pattern)
        elif isinstance(node, ast.MatchOr):
            stack += node.patterns


def _operands(node: ast.AST) -> list[ast.AST]:
    """The sub-expressions whose values an expression may hold or select from."""
    if isinstance(node, (ast.Attribute, ast.NamedExpr)):
        return [node.value]
    if isinstance(node, ast.IfExp):
        return [node.body, node.orelse]
    if isinstance(node, ast.BoolOp):
        return list(node.values)
    return []


def _spelling(callee: ast.AST) -> str:
    """A callee's name and its last four attributes; ``ast.unparse`` recurses
    and an attribute chain can be thousands long."""
    parts: list[str] = []
    while isinstance(callee, ast.Attribute) and len(parts) < 4:
        parts.append(callee.attr)
        callee = callee.value
    head = callee.id if isinstance(callee, ast.Name) else "..."
    return ".".join([head, *reversed(parts)])


def _future_annotations(tree: ast.Module) -> bool:
    return any(
        isinstance(node, ast.ImportFrom)
        and node.module == "__future__"
        and any(alias.name == "annotations" for alias in node.names)
        for node in tree.body
    )


def _environment_writes(
    path: str, source: str, tree: ast.Module, mutable: dict[str, set[str | int]]
) -> list[str]:
    """Each place ``source`` may write the process environment.

    Scopes follow CPython: ``symtable`` says whether a name is local, global or
    free in each function and class body, so a method skips its class body and
    ``global``, ``nonlocal`` and closure names reach the scope that binds them.
    Names are mangled as CPython mangles private names. The walk pairs each
    function, lambda and class with its table by name, line and parameters,
    in CPython's creation order (3.12 creates a try's ``else`` tables before
    its handlers'; those never share a line, so the walk need not). A
    comprehension is a scope of its own (see ``_Scope``); its walrus binds in
    the enclosing function (PEP 572). A scope the walk cannot pair with a
    table, such as a lambda in an annotation under ``from __future__ import
    annotations`` (CPython makes none), is ``unknown``, and so is every scope
    in it: any name read there may hold ``os.environ`` (not a writer
    function), and each write that assumption causes, wherever the value
    flows, says so. A file that never
    spells ``environ``, ``environb``, ``putenv`` or ``unsetenv`` (after NFKC
    normalization, as the parser normalizes identifiers) is not scanned: every
    value the scan follows starts at one of those names.

    A name may hold the os module, the posix or nt module, ``os.environ`` or
    ``os.environb``, or a writer (``putenv``, ``unsetenv``, a bound mutator
    such as ``os.environ.update``). It holds every value any of its bindings
    in that scope gives it: an assignment, an
    annotated assignment, a walrus, a literal tuple or list unpacked element
    by element (one starred target takes the middle), a ``for`` over a literal
    tuple, list or set, a ``with`` target (it may be what the manager was
    given, as ``nullcontext`` returns), a ``case`` capture or ``as`` of the
    whole subject, a parameter default, ``import`` of os, os.path, posix or nt
    (aliased or not) and ``from`` os, posix or nt ``import`` (``*``
    included). An attribute holds every value assigned to an attribute of
    that (mangled) name, or bound to that name in a class body, on any object.

    A write is: storing or deleting an item of the environment, ``|=`` on
    it, calling a writer, passing a writer as an argument, or passing the
    environment to a parameter annotated as a mutable map (by function name;
    see ``_is_mutable_map``; through an attribute, the argument may also
    fill the next parameter, as ``self`` shifts a method's).

    The scan over-approximates: a name holds every value of every binding,
    in any order; attributes match by name alone; a type parameter that
    shadows an alias reads the alias. It does not follow values through
    calls, returns, containers, other modules, sub-patterns of a ``case``
    other than an ``as`` target's capture, a starred item on the right of an
    unpacking, a ``type`` alias's lazy value (``X.__value__``), or reflection
    (``getattr``, ``importlib``, ``sys.modules``, ``ctypes``).
    """
    # The parser normalizes identifiers to NFKC (PEP 3131): `os.ｅnviron`.
    text = source if source.isascii() else unicodedata.normalize("NFKC", source)
    if not any(name in text for name in _ORIGINS):
        return []
    future = _future_annotations(tree)
    try:
        table: symtable.SymbolTable | None = symtable.symtable(source, path, "exec")
    except SyntaxError:  # e.g. `nonlocal` at module level: CPython refuses it
        table = None
    module = _Scope(table, None, "module", unknown=table is None)
    pending: dict[_Scope, dict[_ScopeKey, deque[symtable.SymbolTable]]] = {}
    checks: list[tuple[_Scope, ast.AST]] = []
    homes: dict[tuple[_Scope, str], _Scope] = {}
    # The value graph. A slot is a binding, ("name", scope, name) or
    # ("attribute", name), or one expression, ("expression", id(node)), or one
    # `with` item's given values. Values flow along edges, through an
    # attribute lookup when the edge names one.
    slots: dict[tuple[Any, ...], set[_Value]] = defaultdict(set)
    edges: dict[tuple[Any, ...], list[tuple[tuple[Any, ...], str | None]]]
    edges = defaultdict(list)
    seeds: list[tuple[tuple[Any, ...], _Value]] = []
    built: set[int] = set()

    def register(scope: _Scope) -> _Scope:
        if scope.table is not None:
            tables: dict[_ScopeKey, deque[symtable.SymbolTable]] = defaultdict(deque)
            for nested in _nested_tables(scope.table):
                parameters = (
                    nested.get_parameters()
                    if isinstance(nested, symtable.Function)
                    else ()
                )
                key = _scope_key(nested.get_name(), nested.get_lineno(), parameters)
                tables[key].append(nested)
            pending[scope] = tables
        return scope

    def home(scope: _Scope, name: str) -> _Scope:
        if (scope, name) not in homes:
            homes[scope, name] = scope.home(name)
        return homes[scope, name]

    def bindings(scope: _Scope, name: str) -> tuple[list[tuple[Any, ...]], bool]:
        """The slots a use of ``name`` (mangled) here may read, and whether an
        unknown scope may bind it, so it may hold an assumed ``os.environ``."""
        found: list[tuple[Any, ...]] = []
        owner, unknown = home(scope, name), False
        while True:
            found.append(("name", owner, name))
            if not owner.unknown or owner.parent is None:
                return found, unknown or owner.unknown
            unknown = True
            owner = home(owner.enclosing(name), name)

    def expression(scope: _Scope, root: ast.AST) -> tuple[Any, ...]:
        """The slot of what ``root`` may hold, wired once to the slots it
        reads (one explicit stack: attribute chains and if-else nest deep)."""
        stack = [root]
        while stack:
            node = stack.pop()
            if id(node) in built:
                continue
            built.add(id(node))
            key = ("expression", id(node))
            if isinstance(node, ast.Name):
                keys, unknown = bindings(scope, _mangle(scope.private, node.id))
                for read in keys:
                    edges[read].append((key, None))
                if unknown:
                    seeds.append((key, _Value.ASSUMED_ENVIRON))
            elif isinstance(node, ast.Attribute):
                attribute = ("attribute", _mangle(scope.private, node.attr))
                edges[attribute].append((key, None))
                edges["expression", id(node.value)].append((key, node.attr))
                stack.append(node.value)
            else:
                for part in _operands(node):
                    edges["expression", id(part)].append((key, None))
                    stack.append(part)
        return ("expression", id(root))

    def receives(
        scope: _Scope, target: ast.Name | ast.Attribute
    ) -> list[tuple[Any, ...]]:
        """The slots a binding target fills."""
        if isinstance(target, ast.Attribute):
            return [("attribute", _mangle(scope.private, target.attr))]
        name = _mangle(scope.private, target.id)
        owner = home(scope, name)
        if owner.kind == "class":  # self.X, type(self).X and C.X read it
            return [("name", owner, name), ("attribute", name)]
        return [("name", owner, name)]

    def flow(scope: _Scope, target: ast.AST, origin: _Scope, value: ast.AST) -> None:
        """``target`` in ``scope`` receives ``value`` evaluated in ``origin``."""
        if isinstance(target, (ast.Tuple, ast.List)):
            if not isinstance(value, (ast.Tuple, ast.List)) or any(
                isinstance(item, ast.Starred) for item in value.elts
            ):
                return  # the items are unknown
            starred = [
                index
                for index, element in enumerate(target.elts)
                if isinstance(element, ast.Starred)
            ]
            after = len(target.elts) - starred[0] - 1 if starred else 0
            before = starred[0] if starred else len(target.elts)
            if len(starred) > 1 or len(value.elts) < before + after:
                return
            if not starred and len(value.elts) != before:
                return
            pairs = [
                *zip(target.elts[:before], value.elts[:before]),
                *zip(
                    target.elts[len(target.elts) - after :],
                    value.elts[len(value.elts) - after :],
                ),
            ]
            for element, item in pairs:
                flow(scope, element, origin, item)
            return
        if isinstance(target, (ast.Name, ast.Attribute)):
            source = expression(origin, value)
            for key in receives(scope, target):
                edges[source].append((key, None))

    def bind(scope: _Scope, name: str, held_value: _Value) -> None:
        for key in receives(scope, ast.Name(id=name, ctx=ast.Store())):
            seeds.append((key, held_value))

    def iterate(scope: _Scope, target: ast.AST, origin: _Scope, items: ast.AST) -> None:
        if isinstance(items, (ast.Tuple, ast.List, ast.Set)):
            for item in items.elts:
                if not isinstance(item, ast.Starred):
                    flow(scope, target, origin, item)

    def record(node: ast.AST, scope: _Scope) -> None:
        if isinstance(node, ast.Assign):
            for target in node.targets:
                flow(scope, target, scope, node.value)
        elif isinstance(node, ast.AnnAssign) and node.value is not None:
            flow(scope, node.target, scope, node.value)
        elif isinstance(node, ast.NamedExpr):
            # A comprehension never binds a walrus name (PEP 572), so it
            # resolves to the enclosing scope's.
            flow(scope, node.target, scope, node.value)
        elif isinstance(node, (ast.For, ast.AsyncFor)):
            iterate(scope, node.target, scope, node.iter)
        elif isinstance(node, ast.withitem) and node.optional_vars is not None:
            given = [node.context_expr]
            if isinstance(node.context_expr, ast.Call):
                given += node.context_expr.args
                given += [keyword.value for keyword in node.context_expr.keywords]
            item = ("with", id(node))
            for value in given:
                edges[expression(scope, value)].append((item, None))
            for target in ast.walk(node.optional_vars):
                if isinstance(target, (ast.Name, ast.Attribute)) and isinstance(
                    target.ctx, ast.Store
                ):
                    for key in receives(scope, target):
                        edges[item].append((key, None))
        elif isinstance(node, ast.Match):
            subject = expression(scope, node.subject)
            for case in node.cases:
                for name in _captures(case.pattern):
                    for key in receives(scope, ast.Name(id=name, ctx=ast.Store())):
                        edges[subject].append((key, None))
        elif isinstance(node, ast.Import):
            for alias in node.names:
                root = alias.name.partition(".")[0]
                if root in _MODULES and (alias.asname is None or alias.name == root):
                    bind(scope, alias.asname or root, _MODULES[root])
        elif isinstance(node, ast.ImportFrom) and node.module in _MODULES:
            if node.level:
                return
            for alias in node.names:
                for attribute, held_value in _ATTRIBUTES[_MODULES[node.module]].items():
                    if alias.name in (attribute, "*"):
                        bind(scope, alias.asname or attribute, held_value)
        elif isinstance(node, ast.Subscript) and isinstance(
            node.ctx, (ast.Store, ast.Del)
        ):
            expression(scope, node.value)
            checks.append((scope, node))
        elif isinstance(node, ast.AugAssign) and isinstance(node.op, ast.BitOr):
            expression(scope, node.target)
            checks.append((scope, node))
        elif isinstance(node, ast.Call):
            for part in [node.func, *node.args, *(k.value for k in node.keywords)]:
                expression(scope, part)
            checks.append((scope, node))

    # The walk visits nodes in the order CPython's symtable creates their
    # tables, by one explicit stack, as `expression` and `_is_mutable_map`
    # do: valid code nests attribute chains, if-else, unions
    # and lambdas without bound. What still recurses (unpacking targets,
    # nested comprehensions, `_nested_tables`) nests only through brackets,
    # which CPython's tokenizer stops at 200 levels.
    stack: list[tuple[ast.AST, _Scope] | Callable[[], None]] = []

    def push(scope: _Scope, nodes: Iterable[ast.AST | None]) -> None:
        stack.extend((node, scope) for node in reversed(list(nodes)) if node)

    def annotations_of(scope: _Scope) -> _Scope:
        """Where an annotation is scanned: under ``from __future__ import
        annotations`` CPython never evaluates one and makes no table for a
        scope in it."""
        if not future:
            return scope
        return _Scope(None, scope, "function", scope.private, unknown=True)

    def enter(node: _Function | ast.ClassDef, scope: _Scope) -> None:
        """A function, lambda or class: what its statement evaluates belongs to
        ``scope``, in CPython's order; its body is a new scope."""
        if isinstance(node, ast.ClassDef):
            private, parameters = node.name, []
            defaults: list[tuple[ast.arg, ast.expr]] = []
            parts: list[ast.AST] = [
                *node.decorator_list,
                *node.type_params,
                *node.bases,
                *(keyword.value for keyword in node.keywords),
            ]
            annotations: list[ast.expr] = []
        else:
            args = node.args
            private = scope.private
            positional = [*args.posonlyargs, *args.args]
            defaults = [
                *zip(positional[len(positional) - len(args.defaults) :], args.defaults),
                *(
                    (arg, default)
                    for arg, default in zip(args.kwonlyargs, args.kw_defaults)
                    if default is not None
                ),
            ]
            # CPython annotates *args and **kwargs before keyword-only ones.
            annotated = [*positional, args.vararg, args.kwarg, *args.kwonlyargs]
            parameters = [arg for arg in annotated if arg is not None]
            parts: list[ast.AST] = [default for _, default in defaults]
            if not isinstance(node, ast.Lambda):
                parts += [*node.decorator_list, *node.type_params]
                annotations = [arg.annotation for arg in parameters if arg.annotation]
                annotations += [node.returns] if node.returns else []
            else:
                annotations = []

        def open_body() -> None:
            owner = scope
            while owner.kind == "comprehension" and owner.parent is not None:
                owner = owner.parent
            name = "lambda" if isinstance(node, ast.Lambda) else node.name
            mangled = (_mangle(private, arg.arg) for arg in parameters)
            tables = pending.get(owner, {}).get(_scope_key(name, node.lineno, mangled))
            kind = "class" if isinstance(node, ast.ClassDef) else "function"
            paired = tables.popleft() if tables else None
            inner = register(
                _Scope(paired, scope, kind, private, unknown=paired is None)
            )
            for arg, default in defaults:
                flow(inner, ast.Name(id=arg.arg, ctx=ast.Store()), scope, default)
            push(inner, [node.body] if isinstance(node, ast.Lambda) else node.body)

        stack.append(open_body)
        push(annotations_of(scope), annotations)
        push(scope, parts)

    def comprehension(node: _Comprehension, scope: _Scope) -> None:
        """CPython's order: the first iterable in ``scope``; then each clause's
        target, iterable (not the first's) and conditions; then the value
        before the key, or the element."""
        generators = node.generators
        targets = frozenset(
            _mangle(scope.private, name.id)
            for generator in generators
            for name in ast.walk(generator.target)
            if isinstance(name, ast.Name) and isinstance(name.ctx, ast.Store)
        )
        # Inside an unknown scope a comprehension's names are unknown too.
        inner = _Scope(
            None, scope, "comprehension", scope.private, targets, scope.unknown
        )
        parts: list[ast.AST] = []
        for index, generator in enumerate(generators):
            iterate(inner, generator.target, inner if index else scope, generator.iter)
            parts += [generator.target, *([generator.iter] if index else [])]
            parts += generator.ifs
        if isinstance(node, ast.DictComp):
            parts += [node.value, node.key]
        else:
            parts.append(node.elt)
        push(inner, parts)
        push(scope, [generators[0].iter])

    def visit(node: ast.AST, scope: _Scope) -> None:
        if isinstance(node, _SCOPES):
            enter(node, scope)
        elif isinstance(node, _COMPREHENSIONS):
            comprehension(node, scope)
        elif isinstance(node, ast.AnnAssign):
            record(node, scope)
            push(scope, [node.value])
            push(annotations_of(scope), [node.annotation])
            push(scope, [node.target])
        else:
            record(node, scope)
            push(scope, ast.iter_child_nodes(node))

    push(register(module), tree.body)
    while stack:
        item = stack.pop()
        if isinstance(item, tuple):
            visit(*item)
        else:
            item()

    # Difference propagation: a slot passes on only the values it newly
    # gained, and it gains each value at most once, so the work is linear in
    # the number of edges, which is linear in the program.
    def transform(found: set[_Value], attribute: str) -> set[_Value]:
        result: set[_Value] = set()
        for value in found:
            held_there = _ATTRIBUTES.get(value, {}).get(attribute)
            if held_there is not None:
                result.add(held_there)
            if attribute in _MUTATORS and value is _Value.ENVIRON:
                result.add(_Value.WRITER)
            if attribute in _MUTATORS and value is _Value.ASSUMED_ENVIRON:
                result.add(_Value.ASSUMED_WRITER)
        return result

    queue: deque[tuple[tuple[Any, ...], set[_Value]]] = deque()

    def add(key: tuple[Any, ...], found: set[_Value]) -> None:
        gained = found - slots[key]
        if gained:
            slots[key] |= gained
            queue.append((key, gained))

    for key, seed in seeds:
        add(key, {seed})
    while queue:
        key, gained = queue.popleft()
        for destination, attribute in edges.get(key, ()):
            add(
                destination,
                gained if attribute is None else transform(gained, attribute),
            )

    def holds(node: ast.AST, real: _Value, assumed: _Value) -> str | None:
        """None, or the qualifier a report of ``node`` holding ``real`` needs."""
        found = slots.get(("expression", id(node)), set())
        if real in found:
            return ""
        if assumed in found:
            return " (assumed in a scope not paired with CPython's tables)"
        return None

    writes: list[str] = []

    def note(node: ast.AST, what: str, qualifier: str) -> None:
        line, column = getattr(node, "lineno", 0), getattr(node, "col_offset", 0)
        writes.append(f"{path}:{line}:{column}: {what}{qualifier}")

    environ, writer = (
        (_Value.ENVIRON, _Value.ASSUMED_ENVIRON),
        (_Value.WRITER, _Value.ASSUMED_WRITER),
    )
    for scope, node in checks:
        if isinstance(node, ast.Subscript):
            qualifier = holds(node.value, *environ)
            if qualifier is not None:
                note(node, "assigns or deletes an environment item", qualifier)
            continue
        if isinstance(node, ast.AugAssign):
            qualifier = holds(node.target, *environ)
            if qualifier is not None:
                note(node, "updates the environment with |=", qualifier)
            continue
        assert isinstance(node, ast.Call)
        callee = node.func
        passed = [*node.args, *(keyword.value for keyword in node.keywords)]
        qualifier = holds(callee, *writer)
        if qualifier is not None:
            note(node, f"calls {_spelling(callee)}", qualifier)
        given = sorted(
            qualifier
            for qualifier in (holds(arg, *writer) for arg in passed)
            if qualifier is not None
        )
        if given:
            note(node, f"passes an environment writer to {_spelling(callee)}", given[0])
        name = _named(callee)
        if name is None or name not in mutable:
            continue
        parameters = mutable[name]
        shift = 1 if isinstance(callee, ast.Attribute) else 0
        to_mutable = [
            *(
                arg
                for index, arg in enumerate(node.args)
                if index in parameters or index + shift in parameters
            ),
            *(keyword.value for keyword in node.keywords if keyword.arg in parameters),
        ]
        given = sorted(
            qualifier
            for qualifier in (holds(arg, *environ) for arg in to_mutable)
            if qualifier is not None
        )
        if given:
            note(node, f"passes os.environ to a mutable parameter of {name}", given[0])
    return sorted(set(writes))


def _source_files() -> dict[str, bytes]:
    sources: dict[str, bytes] = {}
    for path in sorted((ROOT / "src" / "molt").rglob("*.py")):
        relative = path.relative_to(ROOT).as_posix()
        if relative.startswith("src/molt/stdlib/"):
            continue  # compiled guest code: os.environ there is the program's
        sources[relative] = path.read_bytes()
    return sources


def _scan(sources: Mapping[str, str | bytes]) -> dict[str, list[str]]:
    # Bytes decode as CPython decodes a file: a BOM or a coding cookie.
    texts = {
        path: source
        if isinstance(source, str)
        else importlib.util.decode_source(source)
        for path, source in sources.items()
    }
    trees = {path: ast.parse(text, path) for path, text in texts.items()}
    mutable = _mutable_parameters(trees)
    return {
        path: _environment_writes(path, texts[path], trees[path], mutable)
        for path in texts
    }


def test_no_cli_module_writes_the_process_environment() -> None:
    writes = _scan(_source_files())
    violations = [
        write
        for path, found in writes.items()
        if path not in _SANCTIONED_WRITERS
        for write in found
    ]

    assert violations == []
    # The sanctioned writers keep the scan honest on real code: each makes
    # three writes (set, pop and restore; two sets and an update).
    assert {path: len(writes[path]) for path in _SANCTIONED_WRITERS} == {
        path: 3 for path in _SANCTIONED_WRITERS
    }


# Each case runs every write site it has once, and each site changes its own
# one-letter variable, so the scan's count, CPython's count of changed
# variables and the count written here agree.
_EXACT_CASES = [
    # The control: CPython's run changes nothing the oracle counts.
    pytest.param("import os\n", 0, id="control"),
    # A module alias written inside a function, and a closure's alias.
    pytest.param(
        "import os\n_ENV = os.environ\ndef f():\n    _ENV['A'] = '1'\nf()\n",
        1,
        id="module-alias",
    ),
    pytest.param(
        "import os\ndef outer():\n    env = os.environ\n"
        "    def inner():\n        env['A'] = '1'\n    inner()\nouter()\n",
        1,
        id="closure-alias",
    ),
    # Defaults, decorators and annotations evaluate in the enclosing scope.
    pytest.param(
        "import os\ndef f(x=os.environ.setdefault('A', '1')):\n    pass\n",
        1,
        id="default",
    ),
    pytest.param(
        "import os\ndef f(*, x=os.environ.setdefault('A', '1')):\n    pass\n",
        1,
        id="keyword-only-default",
    ),
    pytest.param(
        "import os\ndef deco(v):\n    return lambda fn: fn\n"
        "@deco(os.environ.update({'A': '1'}))\ndef f():\n    pass\n",
        1,
        id="decorator",
    ),
    pytest.param(
        # 3.14 evaluates annotations when they are read (PEP 649).
        "import os\ndef f(x: os.environ.setdefault('A', '1')):\n    pass\n"
        "f.__annotations__\n",
        1,
        id="annotation",
    ),
    pytest.param(
        # A closure read by an annotation: 3.14 evaluates it in an annotation
        # scope, and `middle` passes `env` through to it.
        "import os\ndef outer():\n    env = os.environ\n    def middle():\n"
        "        def f(x: env.update({'A': '1'})):\n            pass\n"
        "        f.__annotations__\n    middle()\nouter()\n",
        1,
        id="annotation-closure",
    ),
    pytest.param(
        "import os\nf = lambda: os.environ.update({'A': '1'})\nf()\n",
        1,
        id="lambda",
    ),
    pytest.param(
        "import os\nENV = os.environ\nclass C:\n    def m(self):\n"
        "        ENV.update({'A': '1'})\nC().m()\n",
        1,
        id="method-reads-module-alias",
    ),
    # A sibling's alias, and a local, parameter or `os` that shadows one, are
    # not this function's process environment.
    pytest.param(
        "import os\ndef a():\n    env = os.environ\n"
        "def b(base):\n    env = dict(base)\n    env['A'] = '1'\na()\nb({})\n",
        0,
        id="sibling-alias",
    ),
    pytest.param(
        "import os\nenv = os.environ\ndef c(env):\n    env['A'] = '1'\nc({})\n",
        0,
        id="parameter-shadows",
    ),
    pytest.param(
        "import os\nclass Fake:\n    environ = {}\n"
        "def f(os):\n    os.environ['A'] = '1'\nf(Fake())\n",
        0,
        id="parameter-named-os",
    ),
    # `global` and `nonlocal` names are the enclosing ones, at any depth.
    pytest.param(
        "import os\n_ENV = None\ndef init():\n    global _ENV\n"
        "    _ENV = os.environ\ndef write():\n    _ENV['A'] = '1'\ninit()\nwrite()\n",
        1,
        id="lazy-global-alias",
    ),
    pytest.param(
        "import os\nENV = os.environ\ndef f():\n    global ENV\n"
        "    ENV['A'] = '1'\n    ENV = {}\nf()\n",
        1,
        id="rebound-global",
    ),
    pytest.param(
        "import os\ndef outer():\n    env = os.environ\n    def inner():\n"
        "        nonlocal env\n        env['A'] = '1'\n        env = {}\n"
        "    inner()\nouter()\n",
        1,
        id="nonlocal-write",
    ),
    pytest.param(
        "import os\ndef outer():\n    env = {}\n    def init():\n"
        "        nonlocal env\n        env = os.environ\n    init()\n"
        "    env['A'] = '1'\nouter()\n",
        1,
        id="lazy-nonlocal-alias",
    ),
    pytest.param(
        # Defined in reverse, so each alias appears after its reader: a fixed
        # number of passes would stop short.
        "import os\nA3 = A2 = A1 = None\n"
        "def f3():\n    global A3\n    A3 = A2\n"
        "def f2():\n    global A2\n    A2 = A1\n"
        "def f1():\n    global A1\n    A1 = os.environ\n"
        "def write():\n    A3['A'] = '1'\nf1()\nf2()\nf3()\nwrite()\n",
        1,
        id="global-alias-chain",
    ),
    # A class body is invisible to its methods: a class-level `os` or alias
    # neither hides the module's nor reaches them.
    pytest.param(
        "import os\nclass C:\n    os = None\n    def m(self):\n"
        "        os.environ['A'] = '1'\nC().m()\n",
        1,
        id="class-level-os",
    ),
    pytest.param(
        "import os\nenv = {}\nclass C:\n    env = os.environ\n    def m(self):\n"
        "        env['A'] = '1'\nC().m()\n",
        0,
        id="class-level-alias",
    ),
    pytest.param(
        "import os\ndef outer():\n    env = os.environ\n    class C:\n"
        "        env = {}\n        def m(self):\n            env['A'] = '1'\n"
        "    C().m()\nouter()\n",
        1,
        id="closure-past-class",
    ),
    pytest.param(
        "import os\nENV = os.environ\ndef f():\n    class C:\n"
        "        ENV = {}\n    ENV['A'] = '1'\nf()\n",
        1,
        id="nested-class-binding",
    ),
    # Class attributes, read through the instance, its type or the class.
    pytest.param(
        "import os\nclass C:\n    env = os.environ\n    def m(self):\n"
        "        self.env['A'] = '1'\n        type(self).env['B'] = '1'\n"
        "        C.env['C'] = '1'\nC().m()\n",
        3,
        id="class-attribute",
    ),
    pytest.param(
        "import os\nclass C:\n    def __init__(self):\n"
        "        self.env = os.environ\n    def m(self):\n"
        "        self.env['A'] = '1'\nC().m()\n",
        1,
        id="instance-attribute",
    ),
    # A comprehension's `for` names are its own; its body skips a class
    # body; its walrus binds in the enclosing scope.
    pytest.param(
        "import os\nENV = os.environ\ndef f():\n    names = [ENV for ENV in ['x']]\n"
        "    ENV['A'] = '1'\nf()\n",
        1,
        id="comprehension-name-leaves-alias",
    ),
    pytest.param(
        "import os\nENV = os.environ\n[ENV.update({'A': '1'}) for ENV in [{}]]\n",
        0,
        id="comprehension-name-shadows",
    ),
    pytest.param(
        "import os\nclass C:\n    os = None\n"
        "    done = [os.environ.update({'A': '1'}) for _ in [1]]\n",
        1,
        id="class-comprehension",
    ),
    pytest.param(
        # Its first iterable is evaluated in the class body.
        "import os\nclass C:\n    env = os.environ\n"
        "    done = [e.update({'A': '1'}) for e in [env]]\n",
        1,
        id="class-comprehension-first-iterable",
    ),
    pytest.param(
        "import os\nlist(os.environ.setdefault(k, '1') for k in ['A'])\n",
        1,
        id="generator",
    ),
    pytest.param(
        "import os\n[e := os.environ for _ in [1]]\ne['A'] = '1'\n",
        1,
        id="comprehension-walrus",
    ),
    # Names CPython binds that the scan gives no value: they shadow.
    pytest.param(
        "import os\nENV = os.environ\ndef f(pairs):\n    for ENV, n in pairs:\n"
        "        ENV['A'] = '1'\nf([({}, 1)])\n",
        0,
        id="for-tuple-shadows",
    ),
    pytest.param(
        "import os\nENV = os.environ\nclass E(Exception):\n"
        "    def __setitem__(self, key, value):\n        pass\n"
        "def f():\n    try:\n        raise E\n    except E as ENV:\n"
        "        ENV['A'] = '1'\nf()\n",
        0,
        id="except-shadows",
    ),
    pytest.param(
        "import os\nENV = os.environ\ndef f(value):\n    match value:\n"
        "        case ENV:\n            ENV['A'] = '1'\nf({})\n",
        0,
        id="match-shadows",
    ),
    # Bindings that carry the environment.
    pytest.param(
        "import os\na = os.environ\nb = a\ndef f():\n    b['A'] = '1'\nf()\n",
        1,
        id="alias-of-alias",
    ),
    pytest.param(
        "import os\nenv: dict = os.environ\nenv['A'] = '1'\n", 1, id="annotated"
    ),
    pytest.param(
        "import os\nif (e := os.environ):\n    e['A'] = '1'\n", 1, id="walrus"
    ),
    pytest.param(
        "import os\ne, n = os.environ, 1\ne['A'] = '1'\n", 1, id="tuple-target"
    ),
    pytest.param("import os\n[e] = [os.environ]\ne['A'] = '1'\n", 1, id="list-target"),
    pytest.param(
        "import os\nfor e in [os.environ]:\n    e['A'] = '1'\n", 1, id="for-target"
    ),
    pytest.param(
        "import contextlib, os\nwith contextlib.nullcontext(os.environ) as e:\n"
        "    e['A'] = '1'\n",
        1,
        id="with-target",
    ),
    pytest.param(
        "import os\ndef f(env=os.environ):\n    env['A'] = '1'\nf()\n",
        1,
        id="parameter-default",
    ),
    pytest.param(
        "import os\ndef f(env=None):\n    env = env or os.environ\n"
        "    env['A'] = '1'\nf()\n",
        1,
        id="or-default",
    ),
    # Every spelling of the module, of environ and of the writers.
    pytest.param("import os.path\nos.environ['A'] = '1'\n", 1, id="import-os-path"),
    pytest.param(
        "import os as o\no.environ['A'] = '1'\no.putenv('B', '1')\n",
        2,
        id="import-os-as",
    ),
    pytest.param(
        "from os import environ\nenviron['A'] = '1'\n", 1, id="from-os-environ"
    ),
    pytest.param(
        "from os import environ as e\ne.update({'A': '1'})\n",
        1,
        id="from-os-environ-as",
    ),
    pytest.param(
        "from os import *\nenviron['A'] = '1'\nputenv('B', '1')\n",
        2,
        id="from-os-star",
    ),
    pytest.param("from os import putenv\nputenv('A', '1')\n", 1, id="from-os-putenv"),
    pytest.param("from os import unsetenv as u\nu('D')\n", 1, id="from-os-unsetenv-as"),
    pytest.param("import os\nos.environb[b'A'] = b'1'\n", 1, id="environb"),
    # Every mutator, bound or called, and |=.
    pytest.param(
        "import os\nos.environ.__setitem__('A', '1')\n", 1, id="dunder-setitem"
    ),
    pytest.param("import os\nos.environ.__ior__({'A': '1'})\n", 1, id="dunder-ior"),
    pytest.param("import os\nos.environ.pop('D')\n", 1, id="pop"),
    pytest.param("import os\nos.environ.__delitem__('D')\n", 1, id="dunder-delitem"),
    pytest.param(
        "import os\nwhile 'D' in os.environ:\n    os.environ.popitem()\n",
        1,
        id="popitem",
    ),
    pytest.param("import os\nos.environ.clear()\n", 1, id="clear"),
    pytest.param("import os\nos.environ |= {'A': '1'}\n", 1, id="in-place-or"),
    pytest.param(
        "import os\nalias = os.environ\nalias |= {'A': '1'}\n",
        1,
        id="alias-in-place-or",
    ),
    pytest.param(
        "import os\nset_env = os.environ.__setitem__\nset_env('A', '1')\n",
        1,
        id="bound-mutator",
    ),
    pytest.param(
        "import os\nlist(map(os.environ.pop, ['D']))\n", 1, id="mutator-argument"
    ),
    # A global read only by an annotation: 3.14 lists it in no table of h's.
    pytest.param(
        "import os\nG = os.environ\ndef h():\n    def k(x: G.update({'A': '1'})):\n"
        "        pass\n    k.__annotations__\nh()\n",
        1,
        id="annotation-global",
    ),
    pytest.param(
        "import os\nclass C(os.environ.setdefault('A', '1') and object,\n"
        "        metaclass=os.environ.setdefault('B', '1') and type):\n    pass\n",
        2,
        id="class-bases-and-keywords",
    ),
    # CPython mangles a private name in a class body and every scope in it:
    # `__x` in class `_C` is `_C__x`, `__x__` stays, and class `___` mangles
    # nothing.
    pytest.param(
        "import os\nclass C:\n"
        "    def m(self, __env=os.environ, *__a, __k=1, **__kw):\n"
        "        __env['A'] = '1'\n"
        "        (lambda __e=os.environ: __e.update({'B': '1'}))()\n"
        "        def inner(__e=os.environ):\n            __e['C'] = '1'\n"
        "        inner()\nC().m()\n",
        3,
        id="private-parameters",
    ),
    pytest.param(
        "import os\nclass C:\n    def m(self, __x):\n        local = {}\n"
        "        local['A'] = '1'\n        return os.environ.get('B')\nC().m(1)\n",
        0,
        id="private-parameter-local-write",
    ),
    pytest.param(
        "import os\nclass _C:\n    __env = os.environ\n    __dunder__ = os.environ\n"
        "    def m(self):\n        self.__env['A'] = '1'\n"
        "        type(self).__env['B'] = '1'\n        self.__dunder__['F'] = '1'\n"
        "    @classmethod\n    def n(cls):\n        cls.__env['C'] = '1'\n"
        "_C().m()\n_C.n()\n",
        4,
        id="private-class-attributes",
    ),
    pytest.param(
        "import os\nclass ___:\n    __env = os.environ\n    def m(self):\n"
        "        self.__env['A'] = '1'\n___().m()\n",
        1,
        id="private-name-in-a-class-of-underscores",
    ),
    pytest.param(
        "import os\nclass C:\n    __env = os.environ\nclass D:\n    __env = {}\n"
        "    def m(self):\n        self.__env['A'] = '1'\nD().m()\n",
        0,
        id="private-name-of-another-class",
    ),
    # Under `from __future__ import annotations` CPython makes no table for a
    # lambda in an annotation.
    pytest.param(
        "from __future__ import annotations\nimport os\n"
        "def f(x: (lambda: 0) = (lambda: os.environ.update({'A': '1'}))):\n"
        "    x()\nf()\n",
        1,
        id="future-annotation-lambda",
    ),
    # The parser normalizes identifiers to NFKC (PEP 3131).
    pytest.param(
        "import os\nos.\uff45nviron['A'] = '1'\nos.\uff50utenv('B', '1')\n",
        2,
        id="nfkc-spelling",
    ),
    # Lambdas on one line with one parameter set pair with their tables in
    # CPython's creation order; the walrus makes a swap miss the write.
    pytest.param(
        "import os\ndef outer():\n    env = os.environ\n"
        "    d = {(lambda: (env := {})): (lambda: env.update({'A': '1'})) for _ in [1]}\n"
        "    for value in d.values():\n        value()\nouter()\n",
        1,
        id="order-dict-value-before-key",
    ),
    pytest.param(
        "import os\ndef outer():\n    env = os.environ\n"
        "    def f[T: (lambda: (env := {}))](x: (lambda: env.update({'A': '1'})) = None):\n"
        "        pass\n    f.__annotations__['x']()\nouter()\n",
        1,
        id="order-type-parameter-before-annotation",
    ),
    pytest.param(
        "import os\nclass Base:\n    pass\ndef base(make):\n    make()\n    return Base\n"
        "def outer():\n    env = os.environ\n"
        "    class C[T: (lambda: (env := {}))](base(lambda: env.update({'A': '1'}))):\n"
        "        pass\nouter()\n",
        1,
        id="order-type-parameter-before-base",
    ),
    pytest.param(
        "import os\ndef outer():\n    env = os.environ\n"
        "    def f(*a: (lambda: (env := {})), k: (lambda: env.update({'A': '1'})) = None, "
        "**w: (lambda: (env := {}))):\n"
        "        pass\n    f.__annotations__['k']()\nouter()\n",
        1,
        id="order-varargs-before-keyword-only",
    ),
    pytest.param(
        "import os\ne, *rest = os.environ, 1, 2\ne['A'] = '1'\n"
        "*rest, f = 1, 2, os.environ\nf['B'] = '1'\n",
        2,
        id="starred-target",
    ),
    pytest.param(
        "import os\nmatch os.environ:\n    case {'Z': z}:\n        pass\n"
        "    case e:\n        e['A'] = '1'\nmatch os.environ:\n"
        "    case {} as f:\n        f['B'] = '1'\nmatch os.environ:\n"
        "    case q as e:\n        q['C'] = '1'\n",
        3,
        id="match-capture",
    ),
    pytest.param(
        "import collections.abc, os, typing\n"
        "def a(env: typing.MutableMapping[str, str]) -> None:\n    env['A'] = '1'\n"
        "def b(env: 'dict[str, str] | None') -> None:\n    env['B'] = '1'\n"
        "def c(env: typing.Optional[collections.abc.MutableMapping]) -> None:\n"
        "    env['C'] = '1'\nclass K:\n    def fill(self, env: dict) -> None:\n"
        "        env['E'] = '1'\n"
        "a(os.environ)\nb(os.environ)\nc(os.environ)\nK().fill(os.environ)\n",
        4,
        id="mutable-map-annotations",
    ),
    pytest.param(
        "import posix\nposix.putenv('A', '1')\nfrom posix import unsetenv as u\nu('D')\n",
        2,
        id="posix-writers",
    ),
    pytest.param(
        "import nt\nnt.putenv('A', '1')\nfrom nt import unsetenv as u\nu('D')\n",
        2,
        id="nt-writers",
    ),
    # A global read only by a type parameter's bound, or by a generic
    # function's annotation: no table of h's lists it, on any version.
    pytest.param(
        "import os\nG = os.environ\ndef h():\n"
        "    def k[T: G.update({'A': '1'})](x: G.update({'B': '1'})):\n"
        "        pass\n    k.__type_params__[0].__bound__\n    k.__annotations__\nh()\n",
        2,
        id="type-parameter-global",
    ),
    # Valid code nests without bound: the scan keeps no Python stack frame per
    # level (2,000 levels; CPython 3.12 to 3.15 compile each).
    pytest.param(
        "import os\nclass Node:\n    def __init__(self):\n        self.y = self\n"
        "    def call(self, function):\n        function('A', '1')\nx = Node()\n"
        "x" + ".y" * 2000 + ".call(os.putenv)\n",
        1,
        id="deep-callee",
    ),
    pytest.param(
        "import os\nclass Node:\n    def __init__(self):\n        self.b = self\n"
        "a = Node()\ny = a" + ".b" * 2000 + "\nos.environ.get('A')\n",
        0,
        id="deep-attribute-chain",
    ),
    pytest.param(
        "import os\ny = " + "1 if False else " * 2000 + "os.environ\ny['A'] = '1'\n",
        1,
        id="deep-if-else",
    ),
    pytest.param(
        "import os\ndef f(x: '" + "int | " * 2000 + "dict'):\n    x['A'] = '1'\n"
        "f(os.environ)\n",
        1,
        id="deep-string-union",
    ),
    pytest.param(
        "import os\ndef f(x: " + "int | " * 2000 + "dict):\n    x['A'] = '1'\n"
        "f(os.environ)\n",
        1,
        id="deep-union",
    ),
    pytest.param(
        # CPython's parser runs out of stack on this string, as it would if
        # anything evaluated the annotation.
        "import os\ndef f(x: '" + "-" * 20000 + "1'):\n    pass\nos.environ.get('A')\n",
        0,
        id="unparsable-string-annotation",
    ),
    # Files are bytes: CPython decodes a BOM and a coding cookie.
    pytest.param(b"\xef\xbb\xbfimport os\nos.environ['A'] = '1'\n", 1, id="utf-8-bom"),
    pytest.param(
        "# -*- coding: latin-1 -*-\nimport os\nos.environ['A'] = 'caf\u00e9'\n".encode(
            "latin-1"
        ),
        1,
        id="latin-1-coding-cookie",
    ),
    pytest.param("import os\n(e := os.environ)['A'] = '1'\n", 1, id="walrus-subscript"),
    # A function that reads an attribute before any class binds it.
    pytest.param(
        "import os\ndef w(o):\n    o.env['A'] = '1'\nclass C:\n    env = os.environ\nw(C)\n",
        1,
        id="attribute-read-before-binding",
    ),
]


# Shapes the scan reports though CPython writes nothing: it is flow- and
# path-insensitive and matches attributes by name. A false positive is
# visible in review; a miss is silent.
_CONSERVATIVE_CASES = [
    pytest.param(
        "import os\nenv = os.environ\ndel env\nenv = {}\nenv['A'] = '1'\n",
        1,
        id="rebinding-after-del",
    ),
    pytest.param(
        "import os\nclass Fake:\n    environ = {}\nos = Fake()\nos.environ['A'] = '1'\n",
        1,
        id="os-rebound",
    ),
    pytest.param(
        "import os\nclass Real:\n    env = os.environ\nclass Other:\n    env = {}\n"
        "Other().env['A'] = '1'\n",
        1,
        id="attribute-by-name",
    ),
    pytest.param(
        "import contextlib, os\ndef manager(env):\n    return contextlib.nullcontext({})\n"
        "with manager(os.environ) as e:\n    e['A'] = '1'\n",
        1,
        id="with-target-from-argument",
    ),
    pytest.param(
        "from __future__ import annotations\nimport os\n"
        "def f(x: os.environ.setdefault('A', '1')):\n    pass\n",
        1,
        id="annotation-never-evaluated",
    ),
    pytest.param(
        "import os\nT = os.environ\nclass C[T]:\n    try:\n        T['A'] = '1'\n"
        "    except TypeError:\n        pass\n",
        1,
        id="type-parameter-shadows-alias",
    ),
    # A scope the scan cannot pair: every name in it may hold the environment.
    pytest.param(
        "from __future__ import annotations\nimport os\nENV = os.environ\n"
        "def f(x: (lambda d: d.update({'A': '1'}))):\n    pass\n",
        1,
        id="unpaired-scope",
    ),
    pytest.param(
        "from __future__ import annotations\nimport os\ndef outer():\n"
        "    env = os.environ\n"
        "    x: (lambda: env.update({'A': '1'})) = (lambda: (env := {}))\nouter()\n",
        1,
        id="future-annotation-takes-no-table",
    ),
    pytest.param(
        "from __future__ import annotations\nimport os\nENV = os.environ\n"
        "def f(x: (lambda: [d.update({'A': '1'}) for d in [{}]])):\n    pass\n",
        1,
        id="unpaired-comprehension",
    ),
]


@pytest.mark.parametrize(("source", "sites"), [*_EXACT_CASES, *_CONSERVATIVE_CASES])
def test_the_structure_check_counts_each_case(source: str | bytes, sites: int) -> None:
    writes = _scan({"case.py": source})["case.py"]
    assert len(writes) == sites, writes


def test_a_write_assumed_in_an_unpaired_scope_says_so() -> None:
    assumed = "(assumed in a scope not paired with CPython's tables)"
    in_place = (
        "from __future__ import annotations\nimport os\nENV = os.environ\n"
        "def f(x: (lambda d: d.update({'A': '1'}))):\n    pass\n"
    )
    (write,) = _scan({"case.py": in_place})["case.py"]
    assert write.endswith(assumed)
    # The assumption reaches a paired method through an attribute name.
    carried = (
        "from __future__ import annotations\nimport os\nENV = os.environ\n"
        "class C:\n    def m(self):\n        self.env['A'] = '1'\n"
        "def f(x: [0 for C.env in [d]]):\n    pass\n"
    )
    (write,) = _scan({"case.py": carried})["case.py"]
    assert write.startswith("case.py:6:") and write.endswith(assumed)
    # A module CPython's symtable refuses is scanned unpaired, not dropped.
    refused = "import os\nnonlocal x\nos.environ['A'] = '1'\n"
    (write,) = _scan({"case.py": refused})["case.py"]
    assert write == "case.py:3:0: assigns or deletes an environment item"


def _large_module(count: int) -> str:
    """A module whose `or` chain, `with` targets, chained assignment and `match`
    cases each name ``count`` values; four writes."""
    names = [f"a{index}" for index in range(count)]
    either = " or ".join(names)
    lines = ["import contextlib, os", f"y = {either}"]
    lines += [f"a{index} = a{index + 1}" for index in range(count - 1)]
    lines += [f"a{count - 1} = os.environ", "y['A'] = '1'"]
    targets = ", ".join(f"t{index}" for index in range(count))
    lines += [f"with contextlib.nullcontext({', '.join(names)}) as ({targets}):"]
    lines += ["    t0['B'] = '1'"]
    lines += [" = ".join(f"u{index}" for index in range(count)) + f" = {either}"]
    lines += ["u0['C'] = '1'", f"match {either}:", "    case 0 as m0:"]
    lines += ["        m0['E'] = '1'"]
    for index in range(1, count):
        lines += [f"    case {index} as m{index}:", "        pass"]
    return "\n".join(lines) + "\n"


def test_the_scan_stays_linear_on_a_large_module() -> None:
    """Each expression is one slot, and a slot passes on only the values it
    newly gained. The previous scan re-evaluated whole expressions and made a
    flow per `with` target and argument: on this 3,000-name module it ran for
    minutes and grew to gigabytes; the proof family's deadline ends it."""
    writes = _scan({"large.py": _large_module(3000)})["large.py"]
    assert [write.split(": ", 1)[1] for write in writes] == [
        "assigns or deletes an environment item"
    ] * 4


_WRITE_SHAPES = """
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


def copier(base: dict[str, str]) -> dict[str, str]:
    alias = dict(base)
    alias["I"] = "1"
    return alias


writer()
reader()
copier({})
"""


def test_the_structure_check_finds_each_write_shape() -> None:
    """The scan names each write by its line and kind."""
    writes = _scan({"case.py": _WRITE_SHAPES})["case.py"]
    found = sorted((int(line.split(":")[1]), line.split(": ", 1)[1]) for line in writes)

    # Line 7 writes a parameter, not os.environ: the call site that passes
    # os.environ to it (line 18) is the violation. Line 29 writes a copy that
    # shares only its name with writer's alias.
    assert found == [
        (11, "assigns or deletes an environment item"),
        (12, "calls os.environ.update"),
        (13, "calls os.environ.setdefault"),
        (14, "assigns or deletes an environment item"),
        (15, "calls os.putenv"),
        (17, "assigns or deletes an environment item"),
        (18, "passes os.environ to a mutable parameter of _fill"),
    ]


# The independent oracle. One guarded child runs each case in a fresh CPython
# whose environment is D=0 alone (and SYSTEMROOT, which Windows needs), then
# reads the environment the case leaves through a child of the case: os.putenv
# and os.unsetenv change it without changing os.environ. It reports the
# one-letter variables that changed. A case travels as base64 bytes, which
# CPython decodes as it decodes a file (a BOM, a coding cookie).
_ORACLE_CASE = """
import base64, subprocess, sys
code = base64.b64decode(sys.stdin.read())
exec(compile(code, "case.py", "exec"), {"__name__": "__main__"})
dump = "import json, os; print(json.dumps(dict(os.environ)))"
command = [sys.executable, "-I", "-S", "-c", dump]
sys.stdout.write(subprocess.run(command, capture_output=True, text=True, check=True).stdout)
"""
_ORACLE = """
import json, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

payload = json.load(sys.stdin)
clean = {"D": "0"}
if "SYSTEMROOT" in os.environ:
    clean["SYSTEMROOT"] = os.environ["SYSTEMROOT"]

def changes(source):
    command = [sys.executable, "-I", "-S", "-c", payload["case"]]
    run = subprocess.run(
        command, input=source, env=clean, capture_output=True, text=True, timeout=60
    )
    if run.returncode:
        return run.stderr[-2000:]
    after = json.loads(run.stdout)
    changed = (k for k in clean.keys() | after.keys() if clean.get(k) != after.get(k))
    return sorted(name for name in changed if len(name) == 1)

with ThreadPoolExecutor(4) as pool:
    print(json.dumps(list(pool.map(changes, payload["sources"]))))
"""


# What a case needs from this host's CPython: os.environb and posix are POSIX
# only, nt is Windows only.
_HOST_NEEDS = {
    "environb": hasattr(os, "environb"),
    "import posix": os.name == "posix",
    "import nt": os.name == "nt",
}


def _case(case: Any) -> tuple[str | bytes, int]:
    source, sites = case.values
    assert isinstance(source, (str, bytes)) and isinstance(sites, int)
    return source, sites


def _source_bytes(source: str | bytes) -> bytes:
    return source.encode("utf-8") if isinstance(source, str) else source


def _runs_here(source: str | bytes) -> bool:
    text = _source_bytes(source).decode("latin-1")
    return all(present for needed, present in _HOST_NEEDS.items() if needed in text)


def test_cpython_changes_one_variable_per_counted_write() -> None:
    """The scan's count is CPython's: an exact case changes one variable per
    write site, a conservative case changes none, and the write shapes change
    exactly the variables their seven sites name."""
    cases = [(case, *_case(case)) for case in _EXACT_CASES]
    cases += [(case, _case(case)[0], 0) for case in _CONSERVATIVE_CASES]
    cases = [(case, source, n) for case, source, n in cases if _runs_here(source)]
    sources = [
        base64.b64encode(_source_bytes(source)).decode("ascii")
        for source in [*(source for _, source, _ in cases), _WRITE_SHAPES]
    ]
    result = run_guarded_test_process(
        [sys.executable, "-I", "-B", "-c", _ORACLE],
        env=guard_custody_env(),
        input=json.dumps({"case": _ORACLE_CASE, "sources": sources}),
        check=True,
    )
    assert isinstance(result.stdout, str)
    *changes, shapes = json.loads(result.stdout.splitlines()[-1])

    # A case that raised reports its stderr in place of a list.
    seen = {
        case.id: len(changed) if isinstance(changed, list) else changed
        for (case, _, _), changed in zip(cases, changes, strict=True)
    }
    assert seen == {case.id: count for case, _, count in cases}
    assert shapes == ["A", "B", "C", "D", "E", "F", "X"]
