from __future__ import annotations

import copy
import hashlib
import inspect
import json
from pathlib import Path
import subprocess
from types import SimpleNamespace

import pytest

from molt.cli import llvm_wasi_tools
from molt.cli import native_link_tool_identity
from molt.cli.native_link_plan import (
    NativeLinkCapabilities,
    NativeLinkerKind,
    NativeLinkPlan,
    NativeLinkPolicy,
    NativeObjectFormat,
    NativeTargetSpec,
)
from tools import native_link_benchmark as benchmark


def _plan(tmp_path: Path) -> NativeLinkPlan:
    output = tmp_path / "program.exe"
    obj = tmp_path / "program.obj"
    return NativeLinkPlan(
        target=NativeTargetSpec(
            triple=None,
            os="windows",
            arch="x86_64",
            object_format=NativeObjectFormat.COFF,
        ),
        capabilities=NativeLinkCapabilities(
            linker=NativeLinkerKind.LLD,
            object_format=NativeObjectFormat.COFF,
            explicit_no_icf_flag="identity-policy",
        ),
        policy=NativeLinkPolicy(
            preserve_function_identity=True,
            dead_strip=True,
            emit_relocations=False,
            strip_after_link=False,
            bolt_requested=False,
        ),
        command=("clang", str(obj), "-o", str(output)),
        linker_hint="lld",
        normalized_target=None,
    )


def _report(*, fingerprint_suffix: str = "", warm: list[int] | None = None) -> dict:
    warm = warm or [100, 100, 101, 99, 100]
    runs = [
        {
            "phase": "cold_first",
            "execution": {
                "wall_ns": 200,
                "orchestration_wall_ns": 220,
                "peak_tree_rss_bytes": 1000,
                "peak_job_commit_bytes": 2000,
            },
            "finalization": {"wall_ns": 20},
        },
        *(
            {
                "phase": "warm",
                "execution": {
                    "wall_ns": value,
                    "orchestration_wall_ns": value + 20,
                    "peak_tree_rss_bytes": 900,
                    "peak_job_commit_bytes": 1900,
                },
                "finalization": {"wall_ns": 10},
            }
            for value in warm
        ),
        {
            "phase": "relink",
            "execution": {
                "wall_ns": 110,
                "orchestration_wall_ns": 130,
                "peak_tree_rss_bytes": 950,
                "peak_job_commit_bytes": 1950,
            },
            "finalization": {"wall_ns": 11},
        },
    ]
    identity = {
        field: f"{field}{fingerprint_suffix}" for field in benchmark.IDENTITY_FIELDS
    }
    quiet = {"certified": True, "competing_builds": 0}
    return {
        "identity": identity,
        "quiescence": {"before": quiet, "after": quiet},
        "runs": runs,
        "summary": benchmark.summarize_runs(runs),
    }


def test_input_fingerprint_is_ordered_by_role_and_content_addressed(
    tmp_path: Path,
) -> None:
    left = tmp_path / "left.o"
    right = tmp_path / "right.a"
    left.write_bytes(b"left")
    right.write_bytes(b"right")

    first = benchmark.collect_input_facts({"runtime": right, "object": left})
    second = benchmark.collect_input_facts({"object": left, "runtime": right})
    assert first["fingerprint"] == second["fingerprint"]
    assert [item["role"] for item in first["files"]] == ["object", "runtime"]
    assert first["total_bytes"] == 9

    right.write_bytes(b"drift")
    changed = benchmark.collect_input_facts({"object": left, "runtime": right})
    assert changed["fingerprint"] != first["fingerprint"]


def test_response_files_are_first_class_fingerprinted_inputs(tmp_path: Path) -> None:
    rsp = tmp_path / "link.rsp"
    rsp.write_text("object.o\n", encoding="utf-8")
    assert benchmark.response_file_inputs((f"@{rsp}",)) == {"response_0": rsp}
    assert benchmark.response_file_inputs((f"-Wl,@{rsp}",)) == {"response_0": rsp}


def test_generated_link_scripts_are_first_class_inputs(tmp_path: Path) -> None:
    version_script = tmp_path / "exports.ver"
    def_file = tmp_path / "exports.def"
    version_script.write_text("{ global: main; };\n", encoding="utf-8")
    def_file.write_text("EXPORTS\nmain\n", encoding="utf-8")
    inputs = benchmark.plan_auxiliary_inputs(
        (
            "clang",
            f"-Wl,--version-script={version_script}",
            f"-Wl,/DEF:{def_file}",
        )
    )
    assert set(inputs.values()) == {version_script.resolve(), def_file.resolve()}


def test_explicit_search_directory_libraries_are_content_inputs(tmp_path: Path) -> None:
    library_dir = tmp_path / "native"
    library_dir.mkdir()
    library = library_dir / ("codec.lib" if benchmark.os.name == "nt" else "libcodec.a")
    library.write_bytes(b"archive")
    inputs = benchmark.plan_library_inputs(
        ("clang", f"-L{library_dir}", "-lcodec", "-lsystem_only")
    )
    assert inputs == {"plan_library_0_codec": library.resolve()}


def test_plan_fingerprint_normalizes_workspace_and_fixture_paths(
    tmp_path: Path,
) -> None:
    obj = tmp_path / "program.obj"
    obj.write_bytes(b"obj")
    output = tmp_path / "program.exe"
    payload = benchmark.normalized_plan_payload(
        _plan(tmp_path), inputs={"object": obj}, output=output
    )
    serialized = json.dumps(payload)
    assert str(tmp_path) not in serialized
    assert "{input:object}" in serialized
    assert "{output}" in serialized


def test_plan_allocation_profile_reports_real_tracemalloc_facts(tmp_path: Path) -> None:
    plan, metrics = benchmark.profile_plan(lambda: _plan(tmp_path))
    assert plan.linker_hint == "lld"
    assert metrics["wall_ns"] > 0
    assert metrics["traced_peak_bytes"] >= metrics["traced_current_bytes"]
    assert metrics["net_allocated_blocks"] >= 0
    assert metrics["net_allocated_bytes"] >= 0


def test_link_wall_uses_guarded_child_elapsed_not_orchestration_overhead(
    tmp_path: Path, monkeypatch
) -> None:
    guarded = SimpleNamespace(
        elapsed_s=0.25,
        peak=None,
        peak_total=None,
        peak_job_commit_bytes=4096,
        returncode=0,
        timed_out=False,
    )
    monkeypatch.setattr(
        benchmark.harness_memory_guard,
        "guarded_completed_process",
        lambda *_args, **_kwargs: guarded,
    )
    monkeypatch.setattr(
        benchmark.harness_memory_guard,
        "limits_from_env",
        lambda *_args, **_kwargs: object(),
    )

    result, measurement = benchmark.measure_command(
        ["linker"], cwd=tmp_path, timeout=1.0
    )
    assert result is guarded
    assert measurement["wall_ns"] == 250_000_000
    assert measurement["orchestration_wall_ns"] >= 0
    assert measurement["peak_job_commit_bytes"] == 4096


def test_windows_measurement_fails_closed_without_job_commit(
    tmp_path: Path, monkeypatch
) -> None:
    guarded = SimpleNamespace(
        elapsed_s=0.25,
        peak=None,
        peak_total=None,
        peak_job_commit_bytes=None,
        returncode=0,
        timed_out=False,
    )
    monkeypatch.setattr(benchmark.os, "name", "nt")
    monkeypatch.setattr(
        benchmark.harness_memory_guard,
        "guarded_completed_process",
        lambda *_args, **_kwargs: guarded,
    )
    monkeypatch.setattr(
        benchmark.harness_memory_guard,
        "limits_from_env",
        lambda *_args, **_kwargs: object(),
    )

    with pytest.raises(benchmark.LinkBenchmarkError, match="Job commit"):
        benchmark.measure_command(["linker"], cwd=tmp_path, timeout=1.0)


def test_non_windows_measurement_records_unavailable_job_commit(
    tmp_path: Path, monkeypatch
) -> None:
    guarded = SimpleNamespace(
        elapsed_s=0.25,
        peak=None,
        peak_total=None,
        peak_job_commit_bytes=None,
        returncode=0,
        timed_out=False,
    )
    monkeypatch.setattr(benchmark.os, "name", "posix")
    monkeypatch.setattr(
        benchmark.harness_memory_guard,
        "guarded_completed_process",
        lambda *_args, **_kwargs: guarded,
    )
    monkeypatch.setattr(
        benchmark.harness_memory_guard,
        "limits_from_env",
        lambda *_args, **_kwargs: object(),
    )

    _result, measurement = benchmark.measure_command(
        ["linker"], cwd=tmp_path, timeout=1.0
    )
    assert measurement["peak_job_commit_bytes"] is None


def test_windows_host_report_validation_requires_job_commit_telemetry() -> None:
    report = _report()
    report.update(
        {
            "schema_version": benchmark.SCHEMA_VERSION,
            "kind": benchmark.KIND,
            "host": {"os": "windows"},
            # Host custody remains Windows Job based even for a cross target.
            "target": {"os": "linux", "arch": "x86_64"},
            "inputs": {"count": 3},
            "plan_metrics": {
                "wall_ns": 1,
                "cold_wall_ns": 1,
                "warm_wall_ns_median": 1,
                "warm_wall_ns_mad": 0,
                "traced_peak_bytes": 1,
                "net_allocated_blocks": 1,
                "net_allocated_bytes": 1,
            },
        }
    )
    benchmark.validate_report(report)

    del report["runs"][0]["execution"]["peak_job_commit_bytes"]
    with pytest.raises(benchmark.LinkBenchmarkError, match="run 0.*Job commit"):
        benchmark.validate_report(report)


def test_comparison_rejects_each_identity_drift_class() -> None:
    baseline = _report()
    for field in benchmark.IDENTITY_FIELDS:
        current = copy.deepcopy(baseline)
        current["identity"][field] += "-drift"
        with pytest.raises(benchmark.LinkBenchmarkError, match=field):
            benchmark.compare_reports(baseline, current)


def test_plan_only_and_full_measurements_have_distinct_identity() -> None:
    common = {
        "host": {"fingerprint": "host"},
        "plan_payload": {"plan": 1},
        "inputs": {"fingerprint": "inputs"},
        "tools": {"fingerprint": "tools"},
        "warm_runs": 7,
        "bolt_training_command": None,
    }
    full = benchmark.comparison_identity(**common, measurement_mode="full")
    plan = benchmark.comparison_identity(**common, measurement_mode="plan_only")
    assert full["comparison_fingerprint"] != plan["comparison_fingerprint"]


def test_measurement_authority_is_content_addressed(monkeypatch) -> None:
    monkeypatch.setattr(
        benchmark,
        "_sha256_file",
        lambda path: f"sha:{path.name}",
    )
    first = benchmark.measurement_authority_fingerprint()
    monkeypatch.setattr(
        benchmark,
        "_sha256_file",
        lambda path: f"changed:{path.name}",
    )
    assert benchmark.measurement_authority_fingerprint() != first


def test_implementation_identity_includes_canonical_tool_candidate_resolver() -> None:
    facts = benchmark.implementation_source_facts()
    assert "llvm_wasi_tools.py" in {str(item["name"]) for item in facts["files"]}


def test_comparison_is_attestable_only_for_stable_five_run_warm_samples() -> None:
    baseline = _report()
    current = _report(warm=[90, 91, 90, 89, 90])
    comparison = benchmark.compare_reports(baseline, current)
    assert comparison["attestable"] is True
    assert comparison["phases"]["warm"]["link_wall_ratio"] == pytest.approx(0.9)
    assert comparison["phases"]["warm"]["peak_job_commit_delta_bytes"] == 0

    noisy = _report(warm=[50, 150, 60, 140, 100])
    comparison = benchmark.compare_reports(baseline, noisy)
    assert comparison["attestable"] is False
    assert "descriptive only" in comparison["attestation_reason"]


def test_standalone_attestation_requires_quiescence_and_stability() -> None:
    report = _report()
    report["plan_metrics"] = {"stable": True}
    assert benchmark.report_attestation(report) == {
        "quiescence_certified": True,
        "plan_stable": True,
        "plan_attestable": True,
        "link_stable": True,
        "link_attestable": True,
    }
    report["quiescence"]["after"] = {"certified": False, "competing_builds": 1}
    assert benchmark.report_attestation(report)["plan_attestable"] is False
    assert benchmark.report_attestation(report)["link_attestable"] is False


def test_readobj_parser_counts_structural_records() -> None:
    payload = """
Sections [
  Section {
  }
  Section {
  }
]
Symbols [
  Symbol {
  }
]
Relocations [
  Section (1) .text {
    0x1 IMAGE_REL_AMD64_REL32 symbol
    0x2 IMAGE_REL_AMD64_ADDR64 other
  }
]
"""
    assert benchmark._count_llvm_readobj_records(payload) == {
        "symbols": 1,
        "sections": 2,
        "relocations": 2,
    }


def test_bolt_telemetry_schema_is_fail_closed(tmp_path: Path) -> None:
    path = tmp_path / "bolt.json"
    payload = {
        "schema_version": 1,
        "instrument_wall_ns": 1,
        "train_wall_ns": 2,
        "merge_wall_ns": 3,
        "optimize_wall_ns": 4,
        "profile_fragment_count": 2,
        "profile_fragment_bytes": 99,
    }
    path.write_text(json.dumps(payload), encoding="utf-8")
    assert benchmark._read_bolt_telemetry(path) == payload
    payload.pop("train_wall_ns")
    path.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(benchmark.LinkBenchmarkError, match="train_wall_ns"):
        benchmark._read_bolt_telemetry(path)


def test_named_llvm_tool_uses_canonical_managed_resolution(
    tmp_path: Path, monkeypatch
) -> None:
    managed = tmp_path / "target" / "toolchains" / "llvm-22.1.8" / "bin"
    managed.mkdir(parents=True)
    suffix = ".exe" if llvm_wasi_tools.os.name == "nt" else ""
    readobj = managed / f"llvm-readobj{suffix}"
    readobj.write_bytes(b"readobj")
    monkeypatch.setattr(llvm_wasi_tools.shutil, "which", lambda _name: None)

    candidates = llvm_wasi_tools.llvm_named_tool_candidates(
        "llvm-readobj", target_root=tmp_path / "target"
    )
    assert candidates[0] == readobj.resolve()
    assert readobj.resolve() in candidates


def test_tool_identity_uses_driver_trace_not_print_prog_name_guess(
    tmp_path: Path, monkeypatch
) -> None:
    driver = tmp_path / "clang.exe"
    linker = tmp_path / "lld-link.exe"
    driver.write_bytes(b"driver")
    linker.write_bytes(b"linker")
    plan = _plan(tmp_path)
    plan = NativeLinkPlan(
        target=plan.target,
        capabilities=plan.capabilities,
        policy=plan.policy,
        command=(str(driver), *plan.command[1:]),
        linker_hint=None,
        normalized_target=None,
    )

    def fake_run(command, **_kwargs):
        assert "-###" in command
        return subprocess.CompletedProcess(command, 0, "", f' "{linker}" /OUT:x')

    monkeypatch.setattr(
        native_link_tool_identity.process_guard,
        "run_completed_command",
        fake_run,
    )
    monkeypatch.setattr(native_link_tool_identity, "_tool_version", lambda _path: "v")
    monkeypatch.setattr(
        native_link_tool_identity, "_sha256_file", lambda path: f"hash:{path.name}"
    )
    monkeypatch.setattr(
        native_link_tool_identity, "llvm_named_tool_candidates", lambda *_a, **_k: ()
    )

    facts = native_link_tool_identity.native_link_tool_facts(plan)
    linker_fact = next(fact for fact in facts if fact["role"] == "linker")
    assert linker_fact["path"] == str(linker.resolve())
    assert linker_fact["sha256"] == "hash:lld-link.exe"


def test_tool_identity_rejects_traced_generic_lld_for_coff_role(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    driver = tmp_path / "clang.exe"
    generic = tmp_path / "lld.exe"
    for path in (driver, generic):
        path.write_bytes(path.name.encode())
    base = _plan(tmp_path)
    plan = NativeLinkPlan(
        target=base.target,
        capabilities=base.capabilities,
        policy=base.policy,
        command=(str(driver), *base.command[1:]),
        linker_hint="lld",
        normalized_target=None,
    )
    monkeypatch.setattr(
        native_link_tool_identity,
        "_linker_from_driver_trace",
        lambda _plan, _driver: generic,
    )
    monkeypatch.setattr(
        native_link_tool_identity,
        "llvm_linker_candidates",
        lambda *_args, **_kwargs: pytest.fail(
            "a different exact-role executable cannot attest the traced generic driver"
        ),
    )
    monkeypatch.setattr(
        native_link_tool_identity, "llvm_named_tool_candidates", lambda *_a, **_k: ()
    )

    with pytest.raises(RuntimeError, match="expected lld-link.*generic"):
        native_link_tool_identity.native_link_tool_facts(plan)


def test_link_cache_identity_resolves_exact_object_format_role_without_trace(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    driver = tmp_path / "clang.exe"
    linker = tmp_path / "lld-link.exe"
    driver.write_bytes(b"driver")
    linker.write_bytes(b"linker")
    base = _plan(tmp_path)
    plan = NativeLinkPlan(
        target=base.target,
        capabilities=base.capabilities,
        policy=base.policy,
        command=(str(driver), *base.command[1:]),
        linker_hint="lld",
        normalized_target=None,
    )
    seen: list[str] = []

    def linker_candidates(role: str, **_kwargs: object) -> tuple[Path, ...]:
        seen.append(role)
        return (linker,)

    monkeypatch.setattr(
        native_link_tool_identity, "llvm_linker_candidates", linker_candidates
    )
    monkeypatch.setattr(
        native_link_tool_identity,
        "_linker_from_driver_trace",
        lambda *_args, **_kwargs: pytest.fail(
            "incremental cache identity must not launch the driver trace"
        ),
    )

    facts = native_link_tool_identity.native_link_cache_tool_facts(plan)

    assert seen == ["lld-link"]
    linker_fact = next(fact for fact in facts if fact["role"] == "linker")
    assert linker_fact["path"] == str(linker)
    assert linker_fact["size"] == len(b"linker")


def test_link_benchmark_cannot_reconstruct_link_or_publication_policy() -> None:
    source = inspect.getsource(benchmark)
    assert "_build_native_link_plan(" in source
    assert "_native_link_execution_command(" in source
    assert "_finalize_native_link_candidate(" in source
    assert 'inputs["runtime_link_manifest"]' in source
    assert "native_link_dependency_manifest_path(" in source
    for forbidden in (
        "--gc-sections",
        "/OPT:REF",
        "-dead_strip",
        "_atomic_copy_file(",
        "_assert_native_binary_valid(",
        "native_link_policy_flags(",
    ):
        assert forbidden not in source

    # Fingerprints are versioned, not raw JSON hashes with an implicit schema.
    digest = hashlib.sha256(
        json.dumps(
            {"fingerprint_schema_version": 1, "payload": {"x": 1}},
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=True,
        ).encode("utf-8")
    ).hexdigest()
    assert benchmark._stable_hash({"x": 1}) == digest
