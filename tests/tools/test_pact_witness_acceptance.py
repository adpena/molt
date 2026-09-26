from __future__ import annotations

import json
from pathlib import Path
import subprocess
from types import SimpleNamespace

import pytest

import tools.pact_witness_acceptance as acceptance
from molt.cli.source_extension_set_registry import SourceExtensionVariant
from molt.cli.source_extension_target import resolve_source_extension_target_plan
from molt.node_runtime import NodeRuntime, NodeRuntimeError
from molt.target_python import TargetPythonVersion
from tests.wasm_execution_manifest import write_wasm_execution_manifest


def test_pact_witness_acceptance_rejects_unpinned_provenance(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_WITNESS_EXPECTED_REPO_ROOT", raising=False)
    monkeypatch.delenv("MOLT_WITNESS_EXPECTED_GIT_HEAD", raising=False)

    with pytest.raises(SystemExit, match="provenance is unpinned"):
        acceptance._assert_build_provenance()


def test_pact_witness_node_uses_shared_selection(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    selected = Path("/attested/node")
    calls: list[dict[str, object]] = []

    def resolve(**kwargs: object) -> NodeRuntime:
        calls.append(kwargs)
        return NodeRuntime(selected, "24.0.0", 24)

    monkeypatch.setattr(acceptance, "resolve_node_runtime", resolve)
    assert acceptance._node_bin() == str(selected)
    assert len(calls) == 1
    assert calls[0]["source_root"] == acceptance.ROOT
    assert calls[0]["guard_prefix"] == "MOLT_CROSS"


def test_pact_witness_invalid_node_fails_with_context(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def invalid(**kwargs: object) -> NodeRuntime:
        raise NodeRuntimeError("MOLT_NODE_BIN is invalid")

    monkeypatch.setattr(acceptance, "resolve_node_runtime", invalid)
    with pytest.raises(
        SystemExit, match="Pact witness WASM artifact: MOLT_NODE_BIN is invalid"
    ):
        acceptance._node_bin()


def test_pact_witness_acceptance_attests_pinned_worktree(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setenv("MOLT_WITNESS_EXPECTED_REPO_ROOT", str(acceptance.ROOT))
    monkeypatch.setenv(
        "MOLT_WITNESS_EXPECTED_GIT_HEAD",
        acceptance._git_output("rev-parse", "HEAD"),
    )
    git_output = acceptance._git_output
    monkeypatch.setattr(
        acceptance,
        "_git_output",
        lambda *args: "" if args[0] == "status" else git_output(*args),
    )

    acceptance._assert_build_provenance()

    output = capsys.readouterr().out
    assert f"root={acceptance.ROOT.resolve()}" in output
    assert (
        f"wasm_link={(acceptance.ROOT / 'tools' / 'wasm_link.py').resolve()}" in output
    )


def test_pact_witness_acceptance_check_parity_uses_shared_engine_and_gates(
    tmp_path: Path,
    monkeypatch,
) -> None:
    """`_check_parity` must invoke the ONE shared parity authority --
    `collab/pact/parity/check_parity.py` against the declarative Kernel A
    gate manifest -- not the superseded per-kernel inline oracle at
    `collab/pact/pact_witness_kernel/check_parity.py`. This is the 011
    parity-harness wiring: `tools/pact_witness_acceptance.py` must have
    exactly ONE acceptance authority, never two disagreeing implementations."""
    captured: dict[str, object] = {}

    def fake_run(
        args: list[str], *, cwd: Path, env: dict[str, str] | None = None
    ) -> None:
        captured["args"] = args
        captured["cwd"] = cwd

    monkeypatch.setattr(acceptance, "_run", fake_run)

    candidate = tmp_path / "candidate_outputs.npz"
    reference = tmp_path / "reference_outputs.npz"
    candidate.write_bytes(b"candidate")
    reference.write_bytes(b"reference")

    acceptance._check_parity(candidate, reference, gates=acceptance.KERNEL_A_GATES)

    args = captured["args"]
    assert args[1:] == [
        str(acceptance.PARITY_ENGINE),
        str(candidate),
        str(reference),
        str(acceptance.KERNEL_A_GATES),
    ]
    assert acceptance.PARITY_ENGINE == (
        acceptance.ROOT / "collab" / "pact" / "parity" / "check_parity.py"
    )
    assert acceptance.KERNEL_A_GATES == (
        acceptance.KERNEL_ROOT / "field_solve_gates.json"
    )
    # Never the superseded per-kernel inline oracle (two-arg legacy call
    # shape) -- that would be a second, divergence-prone parity authority.
    assert str(acceptance.KERNEL_ROOT / "check_parity.py") not in args


def test_pact_witness_acceptance_check_parity_requires_reference(
    tmp_path: Path,
) -> None:
    candidate = tmp_path / "candidate_outputs.npz"
    candidate.write_bytes(b"candidate")
    missing_reference = tmp_path / "reference_outputs.npz"

    with pytest.raises(SystemExit, match="missing Pact reference oracle"):
        acceptance._check_parity(
            candidate, missing_reference, gates=acceptance.KERNEL_A_GATES
        )


def test_pact_witness_acceptance_uses_run_scoped_attempt_dirs(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(acceptance, "ROOT", tmp_path)
    monkeypatch.setenv("MOLT_PROOF_QUEUE_RUN_ID", "run:id/with spaces")
    out_dir = tmp_path / "tmp" / "pact_witness_acceptance_queue"
    stale_build = out_dir / "build"
    stale_build.mkdir(parents=True)
    stale_file = stale_build / "output_linked.wat"
    stale_file.write_text(
        "still held by a previous Windows process\n", encoding="utf-8"
    )

    build_dir, run_dir = acceptance._prepare_attempt_dirs(out_dir)
    second_build_dir, second_run_dir = acceptance._prepare_attempt_dirs(out_dir)

    assert build_dir == out_dir / "runs" / "run_id_with_spaces" / "build"
    assert run_dir == out_dir / "runs" / "run_id_with_spaces" / "run"
    assert second_build_dir == out_dir / "runs" / "run_id_with_spaces-2" / "build"
    assert second_run_dir == out_dir / "runs" / "run_id_with_spaces-2" / "run"
    assert stale_file.read_text(encoding="utf-8").startswith("still held")
    assert (out_dir / "latest_attempt.txt").read_text(encoding="utf-8").strip() == str(
        second_build_dir.parent
    )


def test_pact_witness_acceptance_prefers_split_runtime_app_entry(
    tmp_path: Path,
) -> None:
    build_dir = tmp_path / "build"
    build_dir.mkdir()
    output_wasm = build_dir / "output.wasm"
    app_wasm = build_dir / "app.wasm"
    runtime_wasm = build_dir / "molt_runtime.wasm"
    output_wasm.write_bytes(b"monolithic-prelink")
    app_wasm.write_bytes(b"split-app")
    runtime_wasm.write_bytes(b"split-runtime")
    manifest = write_wasm_execution_manifest(
        build_dir, app=app_wasm, runtime=runtime_wasm
    )

    assert acceptance._select_wasm_manifest(build_dir) == manifest


def test_pact_witness_acceptance_uses_output_wasm_without_split_runtime(
    tmp_path: Path,
) -> None:
    build_dir = tmp_path / "build"
    build_dir.mkdir()
    output_wasm = build_dir / "output.wasm"
    output_wasm.write_bytes(b"monolithic")
    manifest = write_wasm_execution_manifest(build_dir, linked=output_wasm)

    assert acceptance._select_wasm_manifest(build_dir) == manifest


def test_pact_witness_acceptance_generates_run_scoped_fixture_and_reference(
    tmp_path: Path,
    monkeypatch,
) -> None:
    kernel_root = tmp_path / "kernel"
    kernel_root.mkdir()
    (kernel_root / "make_fixture.py").write_text(
        "from pathlib import Path\nPath('lstar_sample.npz').write_bytes(b'fixture')\n",
        encoding="utf-8",
    )
    (kernel_root / "field_solve.py").write_text(
        "from pathlib import Path\n"
        "import sys\n"
        "assert Path(sys.argv[1]).read_bytes() == b'fixture'\n"
        "Path('reference_outputs.npz').write_bytes(b'reference')\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(acceptance, "KERNEL_ROOT", kernel_root)
    monkeypatch.setattr(acceptance, "_node_bin", lambda: "node")

    def fake_run_capture(
        args: list[str],
        *,
        cwd: Path,
        env: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        assert args[1] == "--experimental-wasm-exnref"
        assert "wasm/run_wasm.js" in args[2].replace("\\", "/")
        assert (cwd / "lstar_sample.npz").read_bytes() == b"fixture"
        assert (cwd / "reference_oracle.npz").read_bytes() == b"reference"
        (cwd / "reference_outputs.npz").write_bytes(b"candidate")
        return subprocess.CompletedProcess(args, 0, stdout="node ok\n")

    monkeypatch.setattr(acceptance, "_run_capture", fake_run_capture)

    run_dir = tmp_path / "run"
    run_dir.mkdir()
    output_wasm = tmp_path / "output.wasm"
    output_wasm.write_bytes(b"wasm")
    manifest = write_wasm_execution_manifest(tmp_path, linked=output_wasm)

    descriptor = acceptance._ExecutionDescriptor(
        "wasm",
        output_wasm,
        (
            "node",
            "--experimental-wasm-exnref",
            str(acceptance.ROOT / "wasm/run_wasm.js"),
            str(manifest),
        ),
        manifest,
    )
    candidate, reference = acceptance._run_candidate(descriptor, run_dir)

    assert candidate == run_dir / "candidate_outputs.npz"
    assert reference == run_dir / "reference_oracle.npz"
    assert candidate.read_bytes() == b"candidate"
    assert reference.read_bytes() == b"reference"
    assert not (run_dir / "reference_outputs.npz").exists()
    assert not (kernel_root / "lstar_sample.npz").exists()


@pytest.mark.parametrize(
    "target,suffix", [("native", ".molt.a"), ("wasm", ".molt.wasm")]
)
def test_pact_witness_acceptance_reports_static_extension_capsule_drift(
    tmp_path: Path,
    target: str,
    suffix: str,
) -> None:
    module_root = tmp_path / "site"
    manifest_path = (
        module_root / "scipy" / "ndimage" / f"_nd_image{suffix}.extension_manifest.json"
    )
    manifest_path.parent.mkdir(parents=True)
    source_path = tmp_path / "scipy" / "ndimage" / "src" / "nd_image.c"
    source_path.parent.mkdir(parents=True)
    source_path.write_text(
        "static int _nd_image_module_exec(PyObject *module) {\n"
        "    if (_import_array() < 0) { return -1; }\n"
        "    return 0;\n"
        "}\n",
        encoding="utf-8",
    )
    manifest_path.write_text(
        json.dumps(
            {
                "module": "scipy.ndimage._nd_image",
                "extension": f"_nd_image{suffix}",
                "init_symbol": "PyInit__nd_image",
                "runtime_linkage": "static_link",
                "artifact_kind": "wasm_relocatable_object",
                "sources": [str(source_path)],
                "object_closure": {
                    "defined_symbols": [],
                    "runtime_symbols": [],
                    "undefined_symbols": [],
                },
            }
        ),
        encoding="utf-8",
    )
    output_text = (
        "Error: Unhandled Molt exception: ImportError: _nd_image: "
        "static-link PyModuleDef Py_mod_exec slot returned non-zero\n"
    )

    report = acceptance._static_extension_init_failure_report(
        output_text=output_text,
        env={"MOLT_MODULE_ROOTS": str(module_root)},
        target=target,
    )

    assert report is not None
    assert report["failure"]["module"] == "_nd_image"
    match = report["manifest_matches"][0]
    assert match["manifest_path"] == str(manifest_path.resolve())
    assert match["manifest_module"] == "scipy.ndimage._nd_image"
    assert match["missing_manifest_required_capsules"] == [
        "numpy.core._multiarray_umath._ARRAY_API"
    ]
    assert match["source_required_capsules"] == [
        "numpy.core._multiarray_umath._ARRAY_API"
    ]
    assert match["sources"][0]["line_hits"][0]["hits"][0]["line"] == 2
    assert match["sources"][0]["line_hits"][0]["hits"][0]["token"] == "_import_array"


def test_pact_witness_acceptance_writes_static_extension_diagnostic(
    tmp_path: Path,
) -> None:
    module_root = tmp_path / "site"
    manifest_path = module_root / "_native.molt.wasm.extension_manifest.json"
    manifest_path.parent.mkdir(parents=True)
    source_path = tmp_path / "native.c"
    source_path.write_text("int ready(void) { return import_array1(-1); }\n")
    manifest_path.write_text(
        json.dumps(
            {
                "module": "_native",
                "init_symbol": "PyInit__native",
                "sources": [str(source_path)],
                "object_closure": {"required_capsules": []},
            }
        ),
        encoding="utf-8",
    )
    run_dir = tmp_path / "run"
    run_dir.mkdir()

    report_path = acceptance._write_static_extension_init_failure_diagnostic(
        output_text=(
            "ImportError: _native: static-link PyModuleDef "
            "Py_mod_exec slot returned non-zero\n"
        ),
        run_dir=run_dir,
        env={"MOLT_MODULE_ROOTS": str(module_root)},
        target="wasm",
    )

    assert report_path == run_dir / "static_extension_init_failure.json"
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert report["manifest_matches"][0]["missing_manifest_required_capsules"] == [
        "numpy.core._multiarray_umath._ARRAY_API"
    ]


def test_pact_witness_acceptance_diagnoses_numpy_wrapped_static_extension_error(
    tmp_path: Path,
) -> None:
    module_root = tmp_path / "site"
    manifest_path = module_root / "_multiarray_umath.molt.wasm.extension_manifest.json"
    manifest_path.parent.mkdir(parents=True)
    manifest_path.write_text(
        json.dumps(
            {
                "module": "_multiarray_umath",
                "init_symbol": "PyInit__multiarray_umath",
                "sources": [],
                "object_closure": {"required_capsules": []},
            }
        ),
        encoding="utf-8",
    )

    report = acceptance._static_extension_init_failure_report(
        output_text=(
            "Error: Unhandled Molt exception: ImportError:\n\n"
            "Original error was: _multiarray_umath: static-link PyModuleDef "
            "Py_mod_exec slot returned non-zero without setting an exception\n"
        ),
        env={"MOLT_MODULE_ROOTS": str(module_root)},
        target="wasm",
    )

    assert report is not None
    assert report["failure"] == {
        "module": "_multiarray_umath",
        "reason": (
            "static-link PyModuleDef Py_mod_exec slot returned non-zero "
            "without setting an exception"
        ),
    }
    assert report["manifest_matches"][0]["manifest_module"] == "_multiarray_umath"


def test_reference_oracle_pins_numpy_dispatch_baseline(
    tmp_path: Path,
    monkeypatch,
) -> None:
    """ORACLE DETERMINISM PIN (E1 parity feasibility): the numpy-fp32
    reference must be generated on the numpy wheel's portable BASELINE
    dispatch tier (`NPY_DISABLE_CPU_FEATURES=X86_V3`) so the oracle's
    numerics are an attested choice rather than host-CPU luck.

    MASK-PROOF: the pin was measured to be a bitwise NO-OP on the acceptance
    host (all 26 pipeline stages identical with X86_V3 on vs off — see
    docs/agent/E1_PARITY_FEASIBILITY.md), so it cannot absorb a candidate
    divergence; it only removes oracle host-variance. The pin uses
    `setdefault`, so an operator override in the environment wins."""
    kernel_root = tmp_path / "kernel"
    kernel_root.mkdir()
    (kernel_root / "make_fixture.py").write_text("", encoding="utf-8")
    (kernel_root / "field_solve.py").write_text("", encoding="utf-8")
    monkeypatch.setattr(acceptance, "KERNEL_ROOT", kernel_root)

    captured_envs: list[dict[str, str]] = []

    def fake_run(
        args: list[str], *, cwd: Path, env: dict[str, str] | None = None
    ) -> None:
        captured_envs.append(dict(env or {}))
        script = Path(args[1]).name
        if script == "make_fixture.py":
            (cwd / "lstar_sample.npz").write_bytes(b"fixture")
        elif script == "field_solve.py":
            (cwd / "reference_outputs.npz").write_bytes(b"reference")

    monkeypatch.setattr(acceptance, "_run", fake_run)
    monkeypatch.delenv("NPY_DISABLE_CPU_FEATURES", raising=False)

    run_dir = tmp_path / "run"
    run_dir.mkdir()
    reference = acceptance._prepare_reference_oracle(run_dir)

    assert reference == run_dir / "reference_oracle.npz"
    assert len(captured_envs) == 2  # make_fixture + field_solve
    for env in captured_envs:
        assert env.get("NPY_DISABLE_CPU_FEATURES") == "X86_V3"

    # Operator override wins (setdefault semantics), never silently clobbered.
    captured_envs.clear()
    monkeypatch.setenv("NPY_DISABLE_CPU_FEATURES", "")
    acceptance._prepare_reference_oracle(run_dir)
    assert [env.get("NPY_DISABLE_CPU_FEATURES") for env in captured_envs] == ["", ""]


def test_oracle_selfcheck_lane_pins_numpy_dispatch_baseline(monkeypatch) -> None:
    """`tools/pact_witness_oracle.py` (the CPython-only oracle self-check
    lane) must generate with the SAME dispatch pin as the acceptance oracle
    (one oracle numerics authority, no second acceptance path)."""
    import tools.pact_witness_oracle as oracle

    captured_envs: list[dict[str, str]] = []

    def fake_run(self, args, *, cwd, check, env):  # noqa: ANN001
        captured_envs.append(dict(env))

    # The oracle runs inside its locked environment; the test is already the
    # process the prepared command targets. The executor is a frozen record, so
    # the guarded run seam is patched on its type.
    from types import SimpleNamespace

    monkeypatch.setattr(
        oracle,
        "source_build_environment",
        lambda *a, **k: SimpleNamespace(active=True),
    )
    monkeypatch.setattr(type(oracle._COMMANDS), "run", fake_run)
    monkeypatch.delenv("NPY_DISABLE_CPU_FEATURES", raising=False)

    assert oracle.main() == 0
    assert len(captured_envs) == 3  # make_fixture + field_solve + check_parity
    for env in captured_envs:
        assert env.get("NPY_DISABLE_CPU_FEATURES") == "X86_V3"


@pytest.mark.parametrize("name", ["acceptance", "oracle"])
def test_witness_refuses_unprepared_environment_without_launching(name, monkeypatch):
    from types import SimpleNamespace
    import tools.pact_witness_oracle as oracle

    module = acceptance if name == "acceptance" else oracle
    calls = []

    def resolve(root, dependency_group, *, provision=False):
        calls.append(provision)
        return SimpleNamespace(active=False)

    monkeypatch.setattr(module, "source_build_environment", resolve)
    monkeypatch.setattr(
        type(module._COMMANDS),
        "run",
        lambda *a, **k: pytest.fail("unprepared witness launched a child"),
    )
    with pytest.raises(SystemExit, match="prepared locked interpreter"):
        module.main(["--target", "native"]) if name == "acceptance" else module.main()
    assert calls == [False]


@pytest.mark.parametrize("dirty", [" M src/molt/compiler.py", "?? new_module.py"])
def test_acceptance_rejects_dirty_source_identity(monkeypatch, dirty):
    monkeypatch.setenv("MOLT_WITNESS_EXPECTED_REPO_ROOT", str(acceptance.ROOT))
    monkeypatch.setenv("MOLT_WITNESS_EXPECTED_GIT_HEAD", "a" * 40)

    def git_output(*args):
        if args == ("rev-parse", "HEAD"):
            return "a" * 40
        if args[0] == "status":
            return dirty
        return str(acceptance.ROOT)

    monkeypatch.setattr(acceptance, "_git_output", git_output)
    with pytest.raises(SystemExit, match="clean source worktree"):
        acceptance._assert_build_provenance()


@pytest.mark.parametrize("iteration", [None, "1", "false", ""])
def test_acceptance_requires_explicit_non_iteration(monkeypatch, iteration):
    if iteration is None:
        monkeypatch.delenv("MOLT_WITNESS_ITERATION", raising=False)
    else:
        monkeypatch.setenv("MOLT_WITNESS_ITERATION", iteration)
    with pytest.raises(SystemExit, match="queue-locked"):
        acceptance._require_non_iteration_mode()


@pytest.mark.parametrize(
    "name",
    [
        "MOLT_RUNTIME_BUILD_PROFILE",
        "MOLT_WASM_CARGO_PROFILE",
        "MOLT_RELEASE_CARGO_PROFILE",
    ],
)
def test_acceptance_rejects_non_shipping_profile(monkeypatch, name):
    for key, value in acceptance.ACCEPTANCE_ENV.items():
        monkeypatch.setenv(key, value)
    monkeypatch.setenv(name, "dev-fast")
    with pytest.raises(SystemExit, match="shipping profile"):
        acceptance._require_non_iteration_mode()


def _acceptance_build_variant(target: str) -> SourceExtensionVariant:
    return SourceExtensionVariant(
        target_python=TargetPythonVersion(3, 13, 0),
        abi_tier="cpython-abi",
        target_triple=resolve_source_extension_target_plan(target).target_triple,
    )


def _acceptance_build_data(target: str, output: Path) -> dict[str, object]:
    return {
        "target": target,
        # The CLI reports None for native/wasm aliases, not a host-derived
        # explicit triple. The producer resolves the alias against the seal.
        "target_triple": None,
        "profile": "release",
        "emit": "wasm" if target == "wasm" else "bin",
        "entry": str(acceptance.KERNEL_ROOT / "field_solve.py"),
        "consumer_output": str(output),
    }


@pytest.mark.parametrize("target", ["native", "wasm"])
def test_acceptance_build_consumes_json_output_not_guessed_filename(
    tmp_path, monkeypatch, target
):
    build_dir = tmp_path / "build"
    build_dir.mkdir()
    output = build_dir / "compiler-selected-name"
    output.write_bytes(b"artifact")
    manifest = None
    if target == "wasm":
        manifest = write_wasm_execution_manifest(build_dir, linked=output)
    variant = _acceptance_build_variant(target)

    def build(command, *, cwd, env):
        assert command[command.index("--target") + 1] == target
        assert command[command.index("--python-version") + 1] == variant.cpython
        assert command[command.index("--build-profile") + 1] == "release"
        assert command[-1] == "--json"
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps(
                {
                    "status": "ok",
                    "data": _acceptance_build_data(target, output),
                }
            ),
            "",
        )

    monkeypatch.setattr(acceptance, "_run_capture", build)
    monkeypatch.setattr(acceptance, "_assert_no_poison_stubs", lambda *args: None)
    monkeypatch.setattr(acceptance, "_node_bin", lambda: "pinned-node")
    descriptor = acceptance._build_target(target, build_dir, variant=variant)
    assert descriptor.target_artifact == output
    assert descriptor.execution_manifest == manifest
    assert descriptor.execution_command == (
        (str(output),)
        if target == "native"
        else (
            "pinned-node",
            "--experimental-wasm-exnref",
            str(acceptance.ROOT / "wasm/run_wasm.js"),
            str(manifest),
        )
    )


@pytest.mark.parametrize("target", ["native", "wasm"])
@pytest.mark.parametrize(
    ("field", "bad_value"),
    [
        ("target", "other-target"),
        ("profile", "dev"),
        ("emit", "obj"),
        ("entry", "collab/pact/pact_witness_kernel/make_fixture.py"),
        ("target_triple", "other-target"),
    ],
)
def test_acceptance_build_rejects_misreported_effective_facts(
    tmp_path, monkeypatch, target, field, bad_value
):
    build_dir = tmp_path / "build"
    build_dir.mkdir()
    output = build_dir / "compiler-selected-name"
    output.write_bytes(b"artifact")
    variant = _acceptance_build_variant(target)
    data = _acceptance_build_data(target, output)
    if field == "target":
        bad_value = "native" if target == "wasm" else "wasm"
    elif field == "target_triple":
        bad_value = (
            resolve_source_extension_target_plan("native").target_triple
            if target == "wasm"
            else "wasm32-wasip1"
        )
    data[field] = bad_value

    def build(command, *, cwd, env):
        return subprocess.CompletedProcess(
            command, 0, json.dumps({"status": "ok", "data": data}), ""
        )

    monkeypatch.setattr(acceptance, "_run_capture", build)
    with pytest.raises(SystemExit, match="Pact witness build"):
        acceptance._build_target(target, build_dir, variant=variant)


def _publication_fixture(tmp_path, target):
    build_dir = tmp_path / "build"
    run_dir = tmp_path / "run"
    build_dir.mkdir()
    run_dir.mkdir()
    binary = build_dir / "program"
    binary.write_bytes(b"built image")
    manifest = None
    if target == "wasm":
        runtime = build_dir / "runtime.wasm"
        runtime.write_bytes(b"runtime image")
        manifest = write_wasm_execution_manifest(build_dir, app=binary, runtime=runtime)
    descriptor = acceptance._ExecutionDescriptor(
        target, binary, (str(binary),), manifest
    )
    candidate = run_dir / "candidate.npz"
    reference = run_dir / "reference.npz"
    gates = run_dir / "gates.json"
    for path in (candidate, reference, gates):
        path.write_bytes(path.name.encode())

    def package_receipt(package):
        return SimpleNamespace(
            validation=SimpleNamespace(
                recorded=SimpleNamespace(
                    package_version="2.5.1" if package == "numpy" else "1.18.0",
                    name="pact-witness",
                )
            ),
            seal=SimpleNamespace(seal_sha256="b" * 64),
            canonical_identity=SimpleNamespace(canonical_sha256="c" * 64),
        )

    seals = SimpleNamespace(
        variant=SimpleNamespace(
            cpython="3.12",
            abi_tier="cpython-abi",
            target_triple="wasm32-wasip1"
            if target == "wasm"
            else "x86_64-pc-windows-msvc",
        ),
        receipt=package_receipt,
    )
    return dict(
        descriptor=descriptor,
        source_sha="a" * 40,
        seals=seals,
        candidate=candidate,
        reference=reference,
        gates=gates,
        attempt_dir=tmp_path,
    )


@pytest.mark.parametrize("target", ["native", "wasm"])
def test_acceptance_publishes_exact_portable_closure_without_copy(tmp_path, target):
    import shutil

    fixture = _publication_fixture(tmp_path, target)
    before = {
        path.relative_to(tmp_path) for path in tmp_path.rglob("*") if path.is_file()
    }
    receipt = acceptance._write_acceptance_receipt(**fixture)
    after = {
        path.relative_to(tmp_path) for path in tmp_path.rglob("*") if path.is_file()
    }
    assert after - before == {Path("acceptance-receipt.json")}
    payload = json.loads(receipt.read_text())
    target_item = next(
        item for item in payload["artifacts"] if item["role"] == "target_artifact"
    )
    assert target_item["path"] == "build/program"
    assert payload["git"] == {"source_sha": "a" * 40}
    moved = tmp_path.parent / (tmp_path.name + "-relocated")
    shutil.copytree(tmp_path, moved)
    assert (
        acceptance.pact_witness_receipt.validate_acceptance_receipt(
            payload, receipt_path=moved / receipt.name
        )
        == ()
    )
    (moved / "build/program").write_bytes(b"tampered")
    assert acceptance.pact_witness_receipt.validate_acceptance_receipt(
        payload, receipt_path=moved / receipt.name
    )


def test_acceptance_never_publishes_invalid_receipt(tmp_path):
    fixture = _publication_fixture(tmp_path, "wasm")
    (tmp_path / "build/runtime.wasm").write_bytes(b"tampered closure")
    with pytest.raises(SystemExit, match="checksum mismatch"):
        acceptance._write_acceptance_receipt(**fixture)
    assert not (tmp_path / "acceptance-receipt.json").exists()


def test_acceptance_parity_failure_cannot_publish(tmp_path, monkeypatch):
    fixture = _publication_fixture(tmp_path, "native")
    for key, value in acceptance.ACCEPTANCE_ENV.items():
        monkeypatch.setenv(key, value)
    monkeypatch.setattr(
        acceptance, "source_build_environment", lambda *a: SimpleNamespace(active=True)
    )
    monkeypatch.setattr(acceptance, "_assert_build_provenance", lambda: "a" * 40)
    monkeypatch.setattr(
        acceptance, "_validated_extension_seals", lambda target: fixture["seals"]
    )
    monkeypatch.setattr(
        acceptance,
        "_prepare_attempt_dirs",
        lambda out: (tmp_path / "build", tmp_path / "run"),
    )
    monkeypatch.setattr(acceptance, "_default_out_dir", lambda: tmp_path)
    monkeypatch.setattr(acceptance, "KERNEL_A_GATES", fixture["gates"])
    monkeypatch.setattr(
        acceptance, "_build_target", lambda *a, **k: fixture["descriptor"]
    )
    monkeypatch.setattr(
        acceptance,
        "_run_candidate",
        lambda *a: (fixture["candidate"], fixture["reference"]),
    )

    def reject(*args, **kwargs):
        raise subprocess.CalledProcessError(1, "parity engine")

    monkeypatch.setattr(acceptance, "_check_parity", reject)
    with pytest.raises(subprocess.CalledProcessError):
        acceptance.main(["--target", "native"])
    assert not (tmp_path / "acceptance-receipt.json").exists()
