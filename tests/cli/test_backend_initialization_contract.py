"""Startup wrappers preserve failures before publishing usable execution state."""

from __future__ import annotations

from molt.cli.config_resolution import _select_codegen_backend


import ast
from copy import deepcopy
from pathlib import Path

import pytest

from molt.cli.backend_ir import (
    _build_entry_main_ops,
    _build_isolate_bootstrap_ops,
    _build_isolate_import_ops,
    _build_version_info_ops,
    _guard_initialization_ops,
    _prepare_backend_ir,
)
from molt.cli.frontend_integration import _integrate_module_frontend_result_with_state
from molt.cli.function_records import (
    FunctionDigestSeed,
    encode_function_body,
    function_body_digest,
)
from molt.cli.cache_keys import FunctionDigests
from molt.cli.models import _FrontendIntegrationState
from molt.cli.output import fail
from molt.frontend import SimpleTIRGenerator
from molt.target_python import _SUPPORTED_TARGET_PYTHON_BY_SHORT


@pytest.mark.parametrize("mutation", [None, "rewrite", "delete", "topology"])
def test_emitted_ir_fences_source_after_bytes_are_staged(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str | None
) -> None:
    from molt.cli import backend_ir
    from molt.cli.module_source import _ModuleSourceCatalog, _ModuleSourceLease
    from molt.cli.python_import_topology import PythonImportTopology

    source = tmp_path / "app.py"
    source.write_text("value = 1\n", encoding="utf-8")
    optional = tmp_path / "optional.py"
    topology = PythonImportTopology()
    assert topology.probe(optional) is None
    catalog = _ModuleSourceCatalog(
        {"app": _ModuleSourceLease.path_backed(source)}, import_topology=topology
    )
    output = tmp_path / "ir.json"
    output.write_bytes(b"previous IR")
    write_bytes = backend_ir.file_publication.atomic_write_bytes
    staged: list[Path] = []

    def stage_bytes(path, payload, *, replace):
        def before_commit(candidate, destination):
            assert candidate.read_bytes() == payload
            assert destination.read_bytes() == b"previous IR"
            staged.append(candidate)
            if mutation == "rewrite":
                source.write_text("value = 2\n", encoding="utf-8")
            elif mutation == "delete":
                source.unlink()
            elif mutation == "topology":
                optional.write_text("value = 3\n", encoding="utf-8")
            replace(candidate, destination)

        write_bytes(path, payload, replace=before_commit)

    monkeypatch.setattr(backend_ir.file_publication, "atomic_write_bytes", stage_bytes)
    error = backend_ir._write_emitted_ir(
        output, {"functions": []}, validate_inputs=catalog.validate
    )
    assert len(staged) == 1 and not staged[0].exists()
    if mutation is None:
        assert error is None
        assert output.read_text(encoding="utf-8") == '{\n  "functions": []\n}\n'
    else:
        assert error is not None
        assert output.read_bytes() == b"previous IR"


def test_bootstrap_guards_setup_before_later_work_without_clearing() -> None:
    ops = _build_isolate_bootstrap_ops(
        code_slot_count=17,
        version_ops=[{"kind": "const_str", "s_value": "3.12", "out": "version"}],
    )
    assert [op["kind"] for op in ops] == [
        "code_slots_init",
        "check_exception",
        "const_str",
        "check_exception",
        "const_none",
        "check_exception",
        "ret",
        "label",
        "const_none",
        "ret",
    ]
    assert {op["value"] for op in ops if op["kind"] == "check_exception"} == {
        ops[-3]["value"]
    }
    assert ops[-1]["args"] != ops[-4]["args"]


def test_entry_guards_include_late_insertions() -> None:
    ops = _build_entry_main_ops(
        entry_init="molt_init_main",
        version_ops=[{"kind": "const_str", "s_value": "3.12", "out": "version"}],
    )
    ops.insert(1, {"kind": "code_slots_init", "value": 5})
    ops.insert(
        -1, {"kind": "call", "s_value": "molt_init_sys", "args": [], "out": "sys"}
    )
    guarded = _guard_initialization_ops(ops)
    assert not any(op["kind"] == "exception_clear" for op in guarded)
    for index, op in enumerate(guarded):
        if op["kind"] in {"call", "code_slots_init", "const_str"}:
            assert guarded[index + 1]["kind"] == "check_exception"


def test_import_setup_and_each_module_call_share_one_failure_exit() -> None:
    ops = _build_isolate_import_ops(
        code_slot_count=17,
        version_ops=_build_version_info_ops(
            target_python=_SUPPORTED_TARGET_PYTHON_BY_SHORT["3.13"]
        ),
        module_init_symbols=[("sys", "molt_init_sys"), ("json", "molt_init_json")],
    )
    assert not any(op["kind"] == "exception_clear" for op in ops)
    assert ops[1] == {"kind": "check_exception", "value": 1}
    for index, op in enumerate(ops[:-3]):
        if op["kind"] in {"call", "module_cache_get", "const_str", "string_eq"}:
            assert ops[index + 1] == {"kind": "check_exception", "value": 1}
    assert ops[-3] == {"kind": "label", "value": 1}
    assert ops[-1] == {"kind": "ret", "args": [ops[-2]["out"]]}
    assert not any(
        left == right and left["kind"] == "check_exception"
        for left, right in zip(ops, ops[1:])
    )


@pytest.mark.parametrize("minor", [12, 13, 14])
def test_import_commits_target_before_module_access_with_unique_values(
    minor: int,
) -> None:
    version = _SUPPORTED_TARGET_PYTHON_BY_SHORT[f"3.{minor}"]
    version_ops = _build_version_info_ops(target_python=version)
    ops = _build_isolate_import_ops(
        code_slot_count=17,
        version_ops=version_ops,
        module_init_symbols=[("token", "molt_init_token")],
    )
    setter = next(
        i
        for i, op in enumerate(ops)
        if op.get("s_value") == "molt_sys_set_version_info"
    )
    assert len(ops[setter]["args"]) == 5
    values = {op["out"]: op for op in ops if "out" in op}
    actual = []
    for argument in ops[setter]["args"]:
        value = values[argument]
        if value["kind"] == "box":
            value = values[value["args"][0]]
        actual.append(
            value["s_value"] if value["kind"] == "const_str" else value["value"]
        )
    assert tuple(actual) == (
        version.major,
        version.minor,
        version.micro,
        version.release,
        version.serial,
    )
    assert setter < next(
        i for i, op in enumerate(ops) if op["kind"] == "module_cache_get"
    )
    assert setter < next(
        i for i, op in enumerate(ops) if op.get("s_value") == "molt_init_token"
    )
    outputs = [op["out"] for op in ops if "out" in op]
    assert len(outputs) == len(set(outputs))


def test_guard_preserves_control_structure_and_uses_fresh_exit_label() -> None:
    body = [
        {"kind": "label", "value": 7},
        {"kind": "if", "args": ["condition"]},
        {"kind": "call", "s_value": "left", "args": []},
        {"kind": "else"},
        {"kind": "call", "s_value": "right", "args": []},
        {"kind": "end_if"},
        {"kind": "ret_void"},
    ]
    before = [dict(op) for op in body]
    guarded = _guard_initialization_ops(body)
    assert body == before
    assert guarded == [
        body[0],
        body[1],
        body[2],
        {"kind": "check_exception", "value": 8},
        body[3],
        body[4],
        {"kind": "check_exception", "value": 8},
        body[5],
        body[6],
        {"kind": "label", "value": 8},
        {"kind": "ret_void"},
    ]


def test_startup_control_projection_uses_exact_registry_wire_kinds() -> None:
    import tomllib
    from pathlib import Path
    from molt.frontend.lowering.op_kinds_generated import SIMPLEIR_STRUCTURAL_KINDS

    root = Path(__file__).resolve().parents[2]
    with (root / "runtime/molt-ir/src/tir/op_kinds.toml").open("rb") as source:
        table = tomllib.load(source)
    assert SIMPLEIR_STRUCTURAL_KINDS == frozenset(
        row["kind"] for row in table["simpleir_control_kind"] if row["structural"]
    )
    assert "if" in SIMPLEIR_STRUCTURAL_KINDS
    assert "IF" not in SIMPLEIR_STRUCTURAL_KINDS
    assert "exception_new_builtin" not in SIMPLEIR_STRUCTURAL_KINDS


def test_serialized_bootstrap_matches_native_signature_fixture() -> None:
    import json
    from pathlib import Path

    root = Path(__file__).resolve().parents[2]
    fixture = json.loads(
        (root / "tests/fixtures/bootstrap_return_abi.json").read_text(encoding="utf-8")
    )
    emitted = {
        "name": "molt_isolate_bootstrap",
        "params": [],
        "return_abi": "value",
        "ops": _build_isolate_bootstrap_ops(code_slot_count=17, version_ops=[]),
    }
    assert json.loads(json.dumps(emitted)) == fixture
    for name in ("molt_main", "molt_host_init"):
        wrapper = _guard_initialization_ops(
            _build_entry_main_ops(
                entry_init=name,
                version_ops=[],
            )
        )
        assert any(op["kind"] == "ret_void" for op in wrapper)
        assert not any(op["kind"] == "ret" for op in wrapper)


@pytest.mark.parametrize("target", ["native", "wasm", "llvm", "rust", "luau", "mlir"])
@pytest.mark.parametrize("target_python", ["3.12", "3.13", "3.14"])
@pytest.mark.parametrize(
    "entry_module,chunked,stdlib_profile",
    [
        ("__main__", False, "micro"),
        ("entry", True, "full"),
    ],
)
def test_module_initializers_solely_own_code_and_globals_publication(
    tmp_path: Path,
    target: str,
    target_python: str,
    entry_module: str,
    chunked: bool,
    stdlib_profile: str,
) -> None:
    """Exercise real frontend publication through complete backend assembly."""
    version = _SUPPORTED_TARGET_PYTHON_BY_SHORT[target_python]
    modules = ("sys", "dependency", entry_module)
    module_graph = {module: tmp_path / f"{module}.py" for module in modules}
    state = _FrontendIntegrationState(functions=[], known_classes={})
    expected_slots = {}
    for module in modules:
        generator = SimpleTIRGenerator(
            module_name=module,
            module_execution_kind="module" if module == entry_module else "imported",
            known_modules=set(modules),
            source_path=f"generated/{module}.py",
            target_python=(version.major, version.minor),
            module_chunking=chunked,
            module_chunk_max_ops=1,
            module_source_role="user",
        )
        generator.visit(ast.parse("first = 1\nsecond = 2\nthird = 3\n"))
        document = generator.to_json(scope="module")
        seeds = tuple(
            FunctionDigestSeed(
                function, function_body_digest(encode_function_body(function))
            )
            for function in document["functions"]
        )
        issue = _integrate_module_frontend_result_with_state(
            state,
            module,
            ir_functions=document["functions"],
            func_code_ids=document["code_slots"],
            function_metadata=document["function_metadata"],
            function_digest_seeds=seeds,
            local_classes={},
        )
        assert issue is None
        init = SimpleTIRGenerator.module_init_symbol(module)
        ops = next(
            function["ops"] for function in state.functions if function["name"] == init
        )
        slots = [op for op in ops if op["kind"] == "code_slot_set"]
        assert len(slots) == 1
        expected_slots[init] = deepcopy(slots[0])

    prepared, error = _prepare_backend_ir(
        validate_inputs=lambda: None,
        entry_module=entry_module,
        module_graph=module_graph,
        logical_source_path_by_module={
            name: str(path) for name, path in module_graph.items()
        },
        module_is_namespace_by_module={name: False for name in module_graph},
        known_modules=set(modules),
        stdlib_allowlist=set(),
        integration_state=state,
        fail=fail,
        json_output=True,
        module_order=modules,
        runtime_import_dispatch_roots=set(modules),
        spawn_enabled=False,
        pgo_profile_summary=None,
        runtime_feedback_summary=None,
        emit_ir_path=None,
        target_python=version,
        stdlib_profile=stdlib_profile,
        target=_select_codegen_backend(target, "auto")[0],
        module_source_role_by_module={name: "user" for name in module_graph},
    )
    assert error is None
    assert prepared is not None
    assert prepared.ir["code_slots"] == state.global_code_ids
    assert sorted(prepared.ir["code_slots"].values()) == list(
        range(len(state.global_code_ids))
    )
    functions = {function["name"]: function for function in prepared.ir["functions"]}
    original = {seed.function["name"]: seed for seed in state.function_digest_seeds}
    retained = {seed.function["name"]: seed for seed in prepared.function_digest_seeds}
    assert original
    for name, seed in original.items():
        final = functions.get(name)
        if final is seed.function:
            assert retained[name] is seed
        else:
            assert name not in retained, "a rewritten body must lose its old digest"
    # Independently recompute all final bytes: a seed from a pre-rewrite body
    # must never silently qualify its replacement at the final consumer.
    owner = FunctionDigests(prepared.function_digest_seeds)
    for function in functions.values():
        assert owner.digest(function) == function_body_digest(
            encode_function_body(function)
        )
    for name, function in functions.items():
        expected = (
            "value"
            if name in {"molt_isolate_bootstrap", "molt_isolate_import"}
            else "void"
        )
        assert function["return_abi"] == expected, name
    for name in (
        "molt_main",
        "molt_host_init",
        "molt_isolate_bootstrap",
        "molt_isolate_import",
    ):
        if name not in functions:
            continue
        ops = functions[name]["ops"]
        assert not any(op["kind"] in {"code_new", "code_slot_set"} for op in ops)
        allocations = [op for op in ops if op["kind"] == "code_slots_init"]
        assert len(allocations) == 1
        assert allocations[0]["value"] == len(state.global_code_ids)
        for index, op in enumerate(ops):
            if op["kind"] == "code_slots_init":
                assert ops[index + 1]["kind"] == "check_exception"

    published = []
    for init, slot in expected_slots.items():
        ops = functions[init]["ops"]
        slots = [op for op in ops if op["kind"] == "code_slot_set"]
        assert slots == [slot]
        assert len(slot["args"]) == 2
        assert slot["s_value"] == init
        assert "value" not in slot
        published.append(slot["s_value"])
        producers = {op["out"]: op for op in ops if "out" in op}
        code = producers[slot["args"][0]]
        globals_dict = producers[slot["args"][1]]
        assert code["kind"] == "code_new"
        assert len(code["args"]) == 9
        assert producers[code["args"][0]]["s_value"].startswith("generated/")
        assert globals_dict["kind"] == "module_get_attr"
        assert producers[globals_dict["args"][1]]["s_value"] == "__dict__"
        enter_index = next(
            index for index, op in enumerate(ops) if op["kind"] == "trace_enter_slot"
        )
        assert ops.index(slot) < enter_index
        assert ops[enter_index]["s_value"] == slot["s_value"]
        assert "value" not in ops[enter_index]
    assert len(set(published)) == len(modules)
    assert sum(
        op["kind"] == "code_slot_set"
        for function in functions.values()
        for op in function["ops"]
    ) == len(modules)


def test_shared_module_code_payload_is_stable_when_an_earlier_module_is_added() -> None:
    source = "def shared(value):\n    return value + 1\nresult = shared(41)\n"
    changed_source = source.replace("value + 1", "value + 2")
    # CPython supplies the observable distinction for the body mutation below;
    # slot numbering is a compiler representation and must not alter that code.
    baseline_namespace: dict[str, object] = {}
    changed_namespace: dict[str, object] = {}
    exec(compile(source, "generated/common.py", "exec"), baseline_namespace)
    exec(compile(changed_source, "generated/common.py", "exec"), changed_namespace)
    assert baseline_namespace["result"] == 42
    assert changed_namespace["result"] == 43

    def integrate(modules: tuple[str, ...], common_source: str):
        state = _FrontendIntegrationState(functions=[], known_classes={})
        documents = {}
        for module in modules:
            generator = SimpleTIRGenerator(
                module_name=module,
                module_execution_kind="module" if module == "common" else "imported",
                known_modules={"earlier", "common"},
                source_path=f"generated/{module}.py",
                target_python=(3, 12),
                module_source_role="user",
            )
            generator.visit(
                ast.parse(
                    common_source
                    if module == "common"
                    else "def extra():\n    return 7\n"
                )
            )
            document = generator.to_json(scope="module")
            documents[module] = deepcopy(document)
            assert sorted(document["code_slots"].values()) == list(
                range(len(document["code_slots"]))
            )
            assert (
                _integrate_module_frontend_result_with_state(
                    state,
                    module,
                    ir_functions=document["functions"],
                    func_code_ids=document["code_slots"],
                    function_metadata=document["function_metadata"],
                    local_classes={},
                )
                is None
            )
        return state, documents

    base, base_documents = integrate(("common",), source)
    expanded, expanded_documents = integrate(("earlier", "common"), source)
    mutated, _ = integrate(("earlier", "common"), changed_source)
    assert base_documents["common"] == expanded_documents["common"]
    common_names = {function["name"] for function in base.functions}
    assert base.functions == [
        function for function in expanded.functions if function["name"] in common_names
    ]
    assert base.functions != [
        function for function in mutated.functions if function["name"] in common_names
    ]
    shifted_by = len(expanded.global_code_ids) - len(base.global_code_ids)
    assert shifted_by > 0
    assert "molt_main" not in expanded.global_code_ids
    assert SimpleTIRGenerator.module_init_symbol("common") in base.global_code_ids
    for owner, slot in base.global_code_ids.items():
        assert expanded.global_code_ids[owner] == slot + shifted_by
    for state in (base, expanded, mutated):
        assert sorted(state.global_code_ids.values()) == list(
            range(len(state.global_code_ids))
        )
        symbols = {function["name"] for function in state.functions}
        calls = [
            op
            for function in state.functions
            for op in function["ops"]
            if op["kind"] in {"call", "call_internal"}
        ]
        runtime_imports = {op["s_value"] for op in calls} - symbols
        assert runtime_imports, "the fixture must exercise direct runtime calls"
        assert runtime_imports.isdisjoint(state.global_code_ids)
        assert all("value" not in op for op in calls)
        for function in state.functions:
            for op in function["ops"]:
                if op["kind"] in {"code_slot_set", "trace_enter_slot"}:
                    assert op["s_value"] in state.global_code_ids
                    assert "value" not in op
