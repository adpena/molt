from __future__ import annotations

import json
from pathlib import Path
import subprocess

import pytest

import tools.pact_witness_acceptance as acceptance


def test_pact_witness_acceptance_rejects_unpinned_provenance(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_WITNESS_EXPECTED_REPO_ROOT", raising=False)
    monkeypatch.delenv("MOLT_WITNESS_EXPECTED_GIT_HEAD", raising=False)

    with pytest.raises(SystemExit, match="provenance is unpinned"):
        acceptance._assert_build_provenance()


def test_pact_witness_acceptance_attests_pinned_worktree(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setenv("MOLT_WITNESS_EXPECTED_REPO_ROOT", str(acceptance.ROOT))
    monkeypatch.setenv(
        "MOLT_WITNESS_EXPECTED_GIT_HEAD",
        acceptance._git_output("rev-parse", "HEAD"),
    )

    acceptance._assert_build_provenance()

    output = capsys.readouterr().out
    assert f"root={acceptance.ROOT.resolve()}" in output
    assert f"wasm_link={(acceptance.ROOT / 'tools' / 'wasm_link.py').resolve()}" in output


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

    def fake_run(args: list[str], *, cwd: Path, env: dict[str, str] | None = None) -> None:
        captured["args"] = args
        captured["cwd"] = cwd

    monkeypatch.setattr(acceptance, "_run", fake_run)

    candidate = tmp_path / "candidate_outputs.npz"
    reference = tmp_path / "reference_outputs.npz"
    candidate.write_bytes(b"candidate")
    reference.write_bytes(b"reference")

    acceptance._check_parity(candidate, reference)

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


def test_pact_witness_acceptance_check_parity_requires_reference(tmp_path: Path) -> None:
    candidate = tmp_path / "candidate_outputs.npz"
    candidate.write_bytes(b"candidate")
    missing_reference = tmp_path / "reference_outputs.npz"

    with pytest.raises(SystemExit, match="missing Pact reference oracle"):
        acceptance._check_parity(candidate, missing_reference)


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

    selected = acceptance._select_wasm_entry(build_dir)
    env = acceptance._wasm_run_env(selected)

    assert selected == app_wasm
    assert env["MOLT_WASM_DIRECT_LINK"] == "1"
    assert env["MOLT_WASM_PREFER_LINKED"] == "0"
    assert env["MOLT_RUNTIME_WASM"] == str(runtime_wasm)


def test_pact_witness_acceptance_uses_output_wasm_without_split_runtime(
    tmp_path: Path,
) -> None:
    build_dir = tmp_path / "build"
    build_dir.mkdir()
    output_wasm = build_dir / "output.wasm"
    output_wasm.write_bytes(b"monolithic")

    selected = acceptance._select_wasm_entry(build_dir)
    env = acceptance._wasm_run_env(selected)

    assert selected == output_wasm
    assert "MOLT_WASM_DIRECT_LINK" not in env
    assert "MOLT_RUNTIME_WASM" not in env


def test_pact_witness_acceptance_generates_run_scoped_fixture_and_reference(
    tmp_path: Path,
    monkeypatch,
) -> None:
    kernel_root = tmp_path / "kernel"
    kernel_root.mkdir()
    (kernel_root / "make_fixture.py").write_text(
        "from pathlib import Path\n"
        "Path('lstar_sample.npz').write_bytes(b'fixture')\n",
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
        assert "wasm/run_wasm.js" in args[1].replace("\\", "/")
        assert (cwd / "lstar_sample.npz").read_bytes() == b"fixture"
        assert (cwd / "reference_oracle.npz").read_bytes() == b"reference"
        (cwd / "reference_outputs.npz").write_bytes(b"candidate")
        return subprocess.CompletedProcess(args, 0, stdout="node ok\n")

    monkeypatch.setattr(acceptance, "_run_capture", fake_run_capture)

    run_dir = tmp_path / "run"
    run_dir.mkdir()
    output_wasm = tmp_path / "output.wasm"
    output_wasm.write_bytes(b"wasm")

    candidate, reference = acceptance._run_candidate(output_wasm, run_dir)

    assert candidate == run_dir / "candidate_outputs.npz"
    assert reference == run_dir / "reference_oracle.npz"
    assert candidate.read_bytes() == b"candidate"
    assert reference.read_bytes() == b"reference"
    assert not (run_dir / "reference_outputs.npz").exists()
    assert not (kernel_root / "lstar_sample.npz").exists()


def test_pact_witness_acceptance_reports_static_extension_capsule_drift(
    tmp_path: Path,
) -> None:
    module_root = tmp_path / "site"
    manifest_path = (
        module_root
        / "scipy"
        / "ndimage"
        / "_nd_image.molt.wasm.extension_manifest.json"
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
                "extension": "_nd_image.molt.wasm",
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

    def fake_subprocess_run(args, *, cwd, check, env):  # noqa: ANN001
        captured_envs.append(dict(env))

    monkeypatch.setattr(oracle.subprocess, "run", fake_subprocess_run)
    monkeypatch.delenv("NPY_DISABLE_CPU_FEATURES", raising=False)

    assert oracle.main() == 0
    assert len(captured_envs) == 3  # make_fixture + field_solve + check_parity
    for env in captured_envs:
        assert env.get("NPY_DISABLE_CPU_FEATURES") == "X86_V3"
