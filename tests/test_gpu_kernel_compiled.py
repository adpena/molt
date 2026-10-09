from __future__ import annotations

import os
import sys

import pytest
from pathlib import Path

from molt.frontend import compile_to_tir
from molt.dx import development_artifact_env

from tests.native_process_guard import run_native_test_process


ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = ROOT / "src"
SESSION_ID = "pytest-gpu-kernel-compiled"


def _native_env() -> dict[str, str]:
    env = development_artifact_env(
        ROOT,
        os.environ,
        session_prefix="gpu-kernel-compiled",
        session_id=SESSION_ID,
        create_dirs=True,
    )
    env["PYTHONPATH"] = str(SRC_DIR)
    env["MOLT_SESSION_ID"] = SESSION_ID
    env["MOLT_BACKEND_DAEMON"] = "0"
    return env


def _gpu_env(*, metal: bool = False) -> dict[str, str]:
    env = _native_env()
    if metal:
        env["MOLT_RUNTIME_GPU_METAL"] = "1"
        env["MOLT_GPU_BACKEND"] = "metal"
        env["MOLT_TRACE_GPU_BACKEND"] = "1"
    return env


def _gpu_env_webgpu() -> dict[str, str]:
    env = _native_env()
    env["MOLT_RUNTIME_GPU_WEBGPU"] = "1"
    env["MOLT_GPU_BACKEND"] = "webgpu"
    env["MOLT_TRACE_GPU_BACKEND"] = "1"
    return env


def test_compiled_gpu_kernel_vector_add_matches_interpreted_semantics(
    tmp_path: Path,
) -> None:
    src_path = tmp_path / "gpu_kernel_smoke.py"
    out_path = tmp_path / "gpu_kernel_smoke"
    src_path.write_text(
        (ROOT / "tests" / "fixtures" / "gpu_launch_semantics.py").read_text(
            encoding="utf-8"
        ),
        encoding="utf-8",
    )

    env = _gpu_env()
    build = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src_path),
            "--target",
            "native",
            "--build-profile",
            "dev",
            "--backend",
            "cranelift",
            "--output",
            str(out_path),
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=600,
    )
    assert build.returncode == 0, build.stdout + build.stderr

    run = run_native_test_process(
        [str(out_path)],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "[11.0, 22.0, 33.0, 44.0]\nlaunch semantics ok"

    # Same admitted image: auto uses compiled Molt CPU, but an explicit missing
    # kernel capability must not quietly execute that CPU implementation.
    for backend in ("cuda", "hip"):
        refused = run_native_test_process(
            [str(out_path)],
            cwd=ROOT,
            env={**env, "MOLT_GPU_BACKEND": backend},
            capture_output=True,
            text=True,
            timeout=60,
        )
        assert refused.returncode != 0, refused.stdout + refused.stderr
        assert "NotImplementedError" in refused.stderr
        assert "Python-kernel descriptor execution is unavailable" in refused.stderr
        assert "launch semantics ok" not in refused.stdout


def test_compiled_gpu_kernel_vector_add_uses_metal_backend_when_enabled(
    tmp_path: Path,
) -> None:
    if sys.platform != "darwin":
        pytest.skip("Metal execution requires macOS")
    src_path = tmp_path / "gpu_kernel_smoke_metal.py"
    out_path = tmp_path / "gpu_kernel_smoke_metal"
    src_path.write_text(
        (Path(__file__).parent / "fixtures" / "gpu_hardware_admission.py").read_text(
            encoding="utf-8"
        ),
        encoding="utf-8",
    )

    env = _gpu_env(metal=True)
    build = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src_path),
            "--target",
            "native",
            "--build-profile",
            "dev",
            "--backend",
            "cranelift",
            "--output",
            str(out_path),
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=600,
    )
    assert build.returncode == 0, build.stdout + build.stderr

    run = run_native_test_process(
        [str(out_path)],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.splitlines() == [
        "[11.0, 22.0, 33.0, 44.0]",
        "hardware admission ok",
    ]
    assert "[molt gpu backend] metal" in run.stderr


def test_compiled_gpu_kernel_vector_add_uses_webgpu_backend_when_enabled(
    tmp_path: Path,
) -> None:
    src_path = tmp_path / "gpu_kernel_smoke_webgpu.py"
    out_path = tmp_path / "gpu_kernel_smoke_webgpu"
    src_path.write_text(
        (Path(__file__).parent / "fixtures" / "gpu_hardware_admission.py").read_text(
            encoding="utf-8"
        ),
        encoding="utf-8",
    )

    env = _gpu_env_webgpu()
    build = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src_path),
            "--target",
            "native",
            "--build-profile",
            "dev",
            "--backend",
            "cranelift",
            "--output",
            str(out_path),
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=600,
    )
    assert build.returncode == 0, build.stdout + build.stderr

    run = run_native_test_process(
        [str(out_path)],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.splitlines() == [
        "[11.0, 22.0, 33.0, 44.0]",
        "hardware admission ok",
    ]
    assert "[molt gpu backend] webgpu" in run.stderr


def test_gpu_kernel_descriptor_is_published_to_code_metadata() -> None:
    ir = compile_to_tir(
        "import molt.gpu as gpu\n"
        "\n"
        "@gpu.kernel\n"
        "def vector_add(a, b, c, n):\n"
        "    tid = gpu.thread_id()\n"
        "    if tid < n:\n"
        "        c[tid] = a[tid] + b[tid]\n"
        "\n"
        "a = gpu.to_device([1.0, 2.0, 3.0, 4.0])\n"
        "b = gpu.to_device([10.0, 20.0, 30.0, 40.0])\n"
        "c = gpu.alloc(4, float)\n"
        "vector_add[1, 4](a, b, c, 4)\n"
    )

    descriptor_set = False
    descriptor_payload = None
    for func in ir["functions"]:
        for index, op in enumerate(func["ops"]):
            if (
                op.get("kind") == "call"
                and op.get("s_value") == "molt_gpu_kernel_descriptor_set"
            ):
                descriptor_set = True
                value_name = op["args"][1]
                for prior in reversed(func["ops"][:index]):
                    if (
                        prior.get("out") == value_name
                        and prior.get("kind") == "const_str"
                    ):
                        descriptor_payload = prior.get("s_value")
                        break
    assert descriptor_set is True
    assert isinstance(descriptor_payload, str)
    assert '"kind":"molt_gpu_kernel"' in descriptor_payload
    assert '"name":"vector_add"' in descriptor_payload
    assert '"symbol":"__main____vector_add"' in descriptor_payload


def test_gpu_descriptor_projects_captured_body_aliases_and_binding_obligations():
    import json
    from molt.frontend.lowering.gpu_kernel_descriptor import project_kernel_ops

    captured = json.loads(
        (Path(__file__).parent / "fixtures" / "gpu_vector_add_ir.json").read_text(
            encoding="utf-8"
        )
    )[0]
    result = project_kernel_ops(captured["params"], captured["ops"])
    assert "unsupported" not in result
    assert result["buffers"] == ["a", "b", "c"]
    assert result["query_bindings"] == [
        {"out": "v92", "path": ["gpu", "thread_id"], "conditional": False}
    ]
    ops = result["ops"]
    assert [op["args"][0] for op in ops if op["kind"] == "index"] == ["a", "b"]
    assert [op["args"][0] for op in ops if op["kind"] == "store_index"] == ["c"]
    assert next(op for op in ops if op["kind"] == "lt")["args"] == ["v92", "n"]
    assert next(op for op in ops if op["kind"] == "if")["args"] == ["v95"]
    bounds = [r for r in result["requirements"] if r["kind"] == "bounds"]
    assert len(bounds) == 3
    assert all(r["index"] == {"kind": "query", "name": "v92"} for r in bounds)
    assert all(r["upper_limits"] == [{"kind": "scalar", "name": "n"}] for r in bounds)


@pytest.mark.parametrize(
    "mutation",
    [
        "foreign_error_target",
        "unbalanced_tail",
        "truth_callback",
        "unconsumed_lookup",
        "conditional_store",
        "missing_result_name",
        "empty_result_name",
        "non_string_result_name",
    ],
)
def test_gpu_descriptor_refuses_unproved_scaffolding_and_side_effects(mutation):
    import json
    from molt.frontend.lowering.gpu_kernel_descriptor import project_kernel_ops

    captured = json.loads(
        (Path(__file__).parent / "fixtures" / "gpu_vector_add_ir.json").read_text(
            encoding="utf-8"
        )
    )[0]
    ops = captured["ops"]
    if mutation == "foreign_error_target":
        next(op for op in ops[2:] if op["kind"] == "check_exception")["value"] = 999
    elif mutation == "unbalanced_tail":
        ops[-1]["kind"] = "ret"
    elif mutation == "truth_callback":
        next(op for op in ops if op["kind"] == "bool")["args"] = ["v85"]
    elif mutation == "unconsumed_lookup":
        i = next(i for i, op in enumerate(ops) if op["kind"] == "call_func")
        ops.insert(
            i,
            {
                "kind": "get_attr_generic_obj",
                "args": ["v90"],
                "s_value": "unconsumed",
                "out": "extra",
            },
        )
    elif mutation == "conditional_store":
        i = next(i for i, op in enumerate(ops) if op["kind"] == "end_if")
        ops.insert(i, {"kind": "store_var", "args": ["v92"], "var": "tid"})
    else:
        # An unused none producer used to admit absent or malformed names into
        # the descriptor's SSA tables without any dangling-use rejection.
        producer = {"kind": "const_none"}
        if mutation == "empty_result_name":
            producer["out"] = ""
        elif mutation == "non_string_result_name":
            producer["out"] = 7
        ops.insert(2, producer)
    assert "unsupported" in project_kernel_ops(captured["params"], ops)


def test_gpu_descriptor_preserves_comparison_result_kind():
    import json
    from molt.frontend.lowering.gpu_kernel_descriptor import project_kernel_ops

    captured = json.loads(
        (Path(__file__).parent / "fixtures" / "gpu_vector_add_ir.json").read_text(
            encoding="utf-8"
        )
    )[0]
    next(op for op in captured["ops"] if op["kind"] == "add")["kind"] = "lt"
    result = project_kernel_ops(captured["params"], captured["ops"])
    assert "strict integral" in result["unsupported"]


def test_gpu_body_origin_requires_complete_typed_module_generation() -> None:
    import ast
    from molt.frontend.lowering.gpu_kernel_descriptor import body_origin_evidence

    source = (
        "from __future__ import annotations\n"
        "DEPENDENCY = 1\n"
        "class Buffer:\n"
        "    def __getitem__(self, index: int):\n"
        "        return DEPENDENCY + index\n"
    )

    def origin(text: str, *, filename: str = "selected.py", target=(3, 12)):
        tree = ast.parse(text, filename=filename)
        return next(iter(body_origin_evidence("molt.gpu", tree, target).values()))

    selected = origin(source)
    # Neither the application pathname nor a comment grants/changes origin.
    assert origin(source, filename="foreign/renamed.py") == selected
    assert origin(source + "# comment only\n") == selected
    for changed in (
        source.replace("DEPENDENCY = 1", "DEPENDENCY = 2"),
        source.replace("return DEPENDENCY + index", "return DEPENDENCY - index"),
        source.replace("index: int", "index: str"),
        source.replace(
            "from __future__ import annotations", "from __future__ import division"
        ),
    ):
        assert origin(changed) != selected
    assert origin(source, target=(3, 13)) != selected
    assert body_origin_evidence("foreign", ast.parse(source), (3, 12)) == {}


@pytest.mark.parametrize("expression", ["guest_callback()", "0.5", "'text'", "b'data'"])
def test_gpu_body_origin_does_not_evaluate_guest_default_expressions(
    expression,
) -> None:
    import ast
    from molt.frontend.lowering.gpu_kernel_descriptor import body_origin_evidence

    tree = ast.parse(f"def unpack_from(format, buffer, offset={expression}): pass")
    assert body_origin_evidence("struct", tree, (3, 12)) == {}


@pytest.mark.parametrize("value", [None, False, True, 0, 37])
def test_gpu_body_origin_admits_only_typed_literal_defaults(value) -> None:
    import ast
    from molt.frontend.lowering.gpu_kernel_descriptor import body_origin_evidence

    tree = ast.parse(f"def unpack_from(format, buffer, offset={value!r}): pass")
    record = next(iter(body_origin_evidence("struct", tree, (3, 12)).values()))
    assert record["defaults"] == [value]
    assert type(record["defaults"][0]) is type(value)


def test_gpu_body_identity_edges_require_internal_publication() -> None:
    import json
    from molt.frontend.lowering.gpu_kernel_descriptor import descriptor_body_symbols

    descriptor = json.dumps({"python_bodies": {"buffer_get": {"symbol": "actual_get"}}})
    literal = {"kind": "const_str", "out": "metadata", "s_value": descriptor}
    assert descriptor_body_symbols([literal]) == frozenset()
    assert (
        descriptor_body_symbols(
            [
                literal,
                {
                    "kind": "call",
                    "s_value": "guest_function",
                    "args": ["kernel", "metadata"],
                },
            ]
        )
        == frozenset()
    )
    assert descriptor_body_symbols(
        [
            literal,
            {
                "kind": "call",
                "s_value": "molt_gpu_kernel_descriptor_set",
                "args": ["kernel", "metadata"],
            },
        ]
    ) == frozenset({"actual_get"})


@pytest.mark.parametrize("name", [None, "", 7])
def test_gpu_descriptor_name_boundaries_refuse_malformed_names(name) -> None:
    import json
    from molt.frontend.lowering.gpu_kernel_descriptor import (
        UnsupportedKernel,
        _integral_certificate,
        descriptor_publications,
        project_kernel_ops,
    )

    # The certificate must not silently skip a malformed, unused definition.
    with pytest.raises(UnsupportedKernel, match="lacks a name"):
        _integral_certificate([], set(), [{"kind": "const", "value": 0, "out": name}])
    with pytest.raises(ValueError):
        descriptor_publications(
            [
                {"kind": "const_str", "s_value": "{}", "out": name},
                {
                    "kind": "call",
                    "s_value": "molt_gpu_kernel_descriptor_set",
                    "args": ["kernel", name],
                },
            ]
        )
    captured = json.loads(
        (Path(__file__).parent / "fixtures" / "gpu_vector_add_ir.json").read_text(
            encoding="utf-8"
        )
    )[0]
    # Rename both ends, so rejection cannot be explained by a dangling alias.
    for op in captured["ops"]:
        if op.get("var") == "tid":
            op["var"] = name
    result = project_kernel_ops(captured["params"], captured["ops"])
    assert "lacks a name" in result["unsupported"]


def test_gpu_numeric_certificate_tracks_alias_memory_and_signed_zero() -> None:
    from molt.frontend.lowering.gpu_kernel_descriptor import (
        UnsupportedKernel,
        _integral_certificate,
    )

    # This is a semantic operation oracle: after writing a sum into initially
    # small storage, a subsequent alias load must carry the sum's larger bound.
    ops = [
        {"kind": "const", "out": "zero", "value": 0},
        {"kind": "index", "out": "a", "args": ["left", "zero"]},
        {"kind": "index", "out": "b", "args": ["right", "zero"]},
        {"kind": "add", "out": "sum", "args": ["a", "b"]},
        {"kind": "store_index", "args": ["out", "zero", "sum"]},
        {"kind": "index", "out": "later", "args": ["out", "zero"]},
        {"kind": "mul", "out": "product", "args": ["later", "later"]},
        {"kind": "store_index", "args": ["out", "zero", "product"]},
    ]
    certificate = _integral_certificate(
        ["left", "right", "out"], {"left", "right", "out"}, ops
    )
    positive = next(
        case for case in certificate["alternatives"] if case["sign"] == "positive"
    )
    assert positive["magnitude_bits"] <= 11  # (2**b + 2**b)**2 < 2**24
    assert certificate["single_thread"] is True
    # A derived difference may become zero even if both inputs were nonzero.
    # Multiplying that +0 by an unknown-sign input would lose signed zero.
    ops[3]["kind"] = "sub"
    ops[6]["args"] = ["later", "a"]
    with pytest.raises(UnsupportedKernel, match="signed-zero"):
        _integral_certificate(["left", "right", "out"], {"left", "right", "out"}, ops)


def test_gpu_numeric_certificate_keeps_independent_multiplications_independent() -> (
    None
):
    from molt.frontend.lowering.gpu_kernel_descriptor import _integral_certificate

    ops = [
        {"kind": "gpu_query", "out": "tid"},
        {"kind": "index", "out": "a", "args": ["left", "tid"]},
        {"kind": "index", "out": "b", "args": ["right", "tid"]},
        *[{"kind": "mul", "out": f"v{i}", "args": ["a", "b"]} for i in range(5)],
        {"kind": "store_index", "args": ["out", "tid", "v4"]},
    ]
    certificate = _integral_certificate(
        ["left", "right", "out"], {"left", "right", "out"}, ops
    )
    assert certificate["single_thread"] is False
    assert certificate["memory_queries"] == ["tid"]
    assert any(case["magnitude_bits"] == 12 for case in certificate["alternatives"])


@pytest.mark.parametrize("drift", [None, "body", "defaults", "target", "duplicate"])
def test_gpu_final_assembly_binds_captured_origins_to_actual_global_code_slots(
    monkeypatch,
    drift,
) -> None:
    import json
    from types import SimpleNamespace
    from molt.cli import cache_fingerprints
    from molt.cli.backend_ir import _finalize_gpu_descriptors
    from molt.cli.module_source import PythonSourceSnapshot
    from molt.frontend.lowering.gpu_kernel_descriptor import (
        GPU_PYTHON_BODY_REFERENCES,
        body_origin_evidence,
    )
    from molt.python_private_names import resolve_python_private_names
    from molt.target_python import (
        _DEFAULT_TARGET_PYTHON_VERSION,
        _parse_source_for_target,
    )

    target = _DEFAULT_TARGET_PYTHON_VERSION
    captured = {
        relative: PythonSourceSnapshot.capture(ROOT / "src/molt" / relative)
        for relative, _ in GPU_PYTHON_BODY_REFERENCES.values()
    }
    monkeypatch.setattr(
        cache_fingerprints,
        "_frontend_semantic_tooling_snapshot",
        lambda: SimpleNamespace(reference_source=captured.__getitem__),
    )
    bodies = []
    for module, (relative, _) in GPU_PYTHON_BODY_REFERENCES.items():
        source = captured[relative]
        tree = resolve_python_private_names(
            _parse_source_for_target(
                source.content, filename=str(source.path), target_python=target
            )
        )
        for evidence in body_origin_evidence(
            module, tree, target.feature_version
        ).values():
            bodies.append(
                {
                    "name": "selected_" + evidence["role"],
                    "params": ["self"],
                    "ops": [],
                    "gpu_body_origin": evidence,
                }
            )
    changed = bodies[0]
    if drift == "body":
        changed["gpu_body_origin"]["body_ast"] = "0" * 64
    elif drift == "defaults":
        changed["gpu_body_origin"]["defaults"] = [91]
    elif drift == "target":
        changed["gpu_body_origin"]["target_python"] = [3, 14]
    elif drift == "duplicate":
        duplicate = json.loads(json.dumps(changed))
        duplicate["name"] += "_duplicate"
        bodies.append(duplicate)
    literal = {
        "kind": "const_str",
        "out": "metadata",
        "s_value": json.dumps({"symbol": "kernel", "code_slot": 0}),
    }
    kernel = {
        "name": "kernel",
        "params": [],
        "ops": [
            literal,
            {
                "kind": "call",
                "s_value": "molt_gpu_kernel_descriptor_set",
                "args": ["callable", "metadata"],
            },
        ],
    }
    # Both module cache tiers transport JSON; optional facts must survive that
    # boundary before final assembly consumes them, without new FunctionIR fields.
    functions = json.loads(json.dumps([kernel, *bodies]))
    slots = {function["name"]: index + 700 for index, function in enumerate(functions)}
    _finalize_gpu_descriptors(functions, target_python=target, global_code_ids=slots)
    descriptor = json.loads(functions[0]["ops"][0]["s_value"])
    assert descriptor["code_slot"] == 700
    assert all("gpu_body_origin" not in function for function in functions)
    if drift is None:
        assert "unsupported" not in descriptor
        assert len(descriptor["python_bodies"]) == 5
        for role, body in descriptor["python_bodies"].items():
            assert body["symbol"] == "selected_" + role
            assert body["code_slot"] == slots[body["symbol"]]
    else:
        assert descriptor["python_bodies"] == {}
        assert "body origin unavailable" in descriptor["unsupported"]


def test_plain_module_does_not_capture_gpu_reference_bodies(monkeypatch) -> None:
    from molt.cli import cache_fingerprints
    from molt.cli.backend_ir import _finalize_gpu_descriptors
    from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

    def forbidden():
        raise AssertionError("ordinary compilation opened GPU reference sources")

    monkeypatch.setattr(
        cache_fingerprints, "_frontend_semantic_tooling_snapshot", forbidden
    )
    functions = [
        {"name": "plain", "ops": [], "gpu_body_origin": {"role": "buffer_get"}}
    ]
    _finalize_gpu_descriptors(
        functions,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
        global_code_ids={"plain": 8},
    )
    assert functions == [{"name": "plain", "ops": []}]


def test_native_support_pruning_preserves_only_unchanged_body_origins() -> None:
    import ast
    from molt.compiler_analysis.native_support_slice import prune_native_support_module
    from molt.frontend.lowering.gpu_kernel_descriptor import (
        body_origin_evidence,
        rebind_pruned_body_origins,
    )

    for decorator in ("", "@guest_decorator\n"):
        tree = ast.parse(
            decorator + "def unpack_from(format, buffer, offset=0): return buffer\n"
        )
        evidence = body_origin_evidence("struct", tree, (3, 12))
        selected, _, missing = prune_native_support_module(tree, {"unpack_from"})
        assert not missing
        assert selected.body[0] is not tree.body[0]
        rebound = rebind_pruned_body_origins(selected, evidence)
        if decorator:
            assert rebound == {}  # Removing a decorator changes executable semantics.
        else:
            assert rebound[id(selected.body[0])] == evidence[id(tree.body[0])]
