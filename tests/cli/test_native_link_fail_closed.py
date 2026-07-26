from __future__ import annotations

import inspect
from pathlib import Path
import subprocess

from molt.cli import backend_output_pipeline, commands, native_toolchain
from molt.cli import link_pipeline


def test_native_link_has_one_attempt_and_no_post_failure_fallback() -> None:
    source = inspect.getsource(link_pipeline._prepare_native_link)
    assert source.count("_run_native_link_command(") == 1
    assert "retry" not in source.lower()
    assert "Linker fallback:" not in source


def test_runtime_source_provenance_is_verified_before_every_production_link() -> None:
    pipeline = inspect.getsource(backend_output_pipeline._emit_backend_pipeline_outputs)
    ensure = pipeline.index("_ensure_native_runtime_lib_ready_before_link(")
    failure = pipeline.index("return_after_build_diagnostics(", ensure)
    prepare = pipeline.index("_prepare_native_link(", failure)
    assert ensure < failure < prepare

    link = inspect.getsource(link_pipeline._prepare_native_link)
    assert "source_root=molt_root" in link
    assert "source_fingerprint=runtime_source_fingerprint" in link


def test_darwin_validation_reports_invalid_selected_linker_output() -> None:
    source = inspect.getsource(link_pipeline._validate_darwin_link_output)
    assert "_run_native_link_command" not in source
    assert "retry" not in source.lower()


def test_bolt_runs_inside_build_before_success_is_emitted() -> None:
    source = inspect.getsource(backend_output_pipeline._emit_backend_pipeline_outputs)
    assert source.index("_run_bolt_post_link(") < source.index(
        "return _emit_native_link_result("
    )


def test_bolt_uses_shared_candidate_finalization_before_success() -> None:
    source = inspect.getsource(native_toolchain._run_bolt_post_link)
    finalize = source.index("_finalize_native_link_candidate(")
    success = source.index("return 0", finalize)
    assert finalize < success


def test_bolt_success_without_optimized_artifact_fails_closed(
    tmp_path, monkeypatch
) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    (tools / "bolt_optimize.sh").write_text("#!/usr/bin/env bash\n", encoding="utf-8")
    binary = tmp_path / "program"
    binary.write_bytes(b"input")
    monkeypatch.setattr(native_toolchain, "_compiler_root", lambda: tmp_path)
    monkeypatch.setattr(native_toolchain.sys, "platform", "linux")
    monkeypatch.setattr(native_toolchain.platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(
        native_toolchain,
        "_run_completed_command",
        lambda *_args, **_kwargs: subprocess.CompletedProcess([], 0, "", ""),
    )
    assert (
        native_toolchain._run_bolt_post_link(
            bolt_requested=True,
            bolt_training_cmd=None,
            target="native",
            output=str(binary),
            out_dir=None,
            build_rc=0,
            json_output=True,
        )
        == 1
    )


def test_bolt_finalizes_candidate_before_atomic_publication(
    tmp_path, monkeypatch
) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    (tools / "bolt_optimize.sh").write_text("#!/usr/bin/env bash\n", encoding="utf-8")
    binary = tmp_path / "program"
    bolt_binary = Path(f"{binary}.bolt")
    binary.write_bytes(b"original")
    bolt_binary.write_bytes(b"optimized")
    received: dict[str, object] = {}
    monkeypatch.setattr(native_toolchain, "_compiler_root", lambda: tmp_path)
    monkeypatch.setattr(native_toolchain.sys, "platform", "linux")
    monkeypatch.setattr(native_toolchain.platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(
        native_toolchain,
        "_run_completed_command",
        lambda *_args, **_kwargs: subprocess.CompletedProcess([], 0, "", ""),
    )

    def fake_finalize(**kwargs: object) -> None:
        received.update(kwargs)
        Path(kwargs["output_binary"]).write_bytes(b"finalized")  # type: ignore[arg-type]
        return None

    monkeypatch.setattr(
        "molt.cli.build_results._finalize_native_link_candidate", fake_finalize
    )

    assert (
        native_toolchain._run_bolt_post_link(
            bolt_requested=True,
            bolt_training_cmd=None,
            target="native",
            output=str(binary),
            out_dir=None,
            build_rc=0,
            json_output=True,
        )
        == 0
    )
    assert received == {
        "candidate": bolt_binary,
        "output_binary": binary,
        "target_triple": None,
        "strip": True,
    }
    assert binary.read_bytes() == b"finalized"


def test_bolt_helper_never_emits_json_outside_build_result_authority(
    tmp_path, monkeypatch, capsys
) -> None:
    binary = tmp_path / "program"
    binary.write_bytes(b"input")
    monkeypatch.setattr(native_toolchain, "_compiler_root", lambda: tmp_path)
    monkeypatch.setattr(native_toolchain.sys, "platform", "linux")
    monkeypatch.setattr(native_toolchain.platform, "machine", lambda: "x86_64")

    assert (
        native_toolchain._run_bolt_post_link(
            bolt_requested=True,
            bolt_training_cmd=None,
            target="native",
            output=str(binary),
            out_dir=None,
            build_rc=0,
            json_output=True,
        )
        == 1
    )
    assert capsys.readouterr().out == ""


def test_bolt_script_merges_every_pid_profile_fragment() -> None:
    script = (
        Path(__file__).resolve().parents[2] / "tools" / "bolt_optimize.sh"
    ).read_text(encoding="utf-8")

    assert 'PROFILE_FRAGMENTS+=("$profile")' in script
    assert 'merge-fdata "${PROFILE_FRAGMENTS[@]}"' in script
    assert '-data="$FDATA_FOUND"' in script


def test_bolt_script_emits_atomic_phase_telemetry_when_requested() -> None:
    script = (
        Path(__file__).resolve().parents[2] / "tools" / "bolt_optimize.sh"
    ).read_text(encoding="utf-8")

    assert 'TELEMETRY_JSON="${MOLT_BOLT_TELEMETRY_JSON:-}"' in script
    for metric in (
        "instrument_wall_ns",
        "train_wall_ns",
        "merge_wall_ns",
        "optimize_wall_ns",
        "profile_fragment_count",
        "profile_fragment_bytes",
    ):
        assert metric in script
    assert 'mv -f -- "$TELEMETRY_TMP" "$TELEMETRY_JSON"' in script
    assert "printf -v INSTRUMENTED_QUOTED '%q'" in script


def test_extension_link_applies_identity_policy_after_user_link_arguments() -> None:
    source = inspect.getsource(commands.extension_build)
    policy = source.index("_source_extension_link_policy_args(")
    user_args = source.rindex("link_command.extend(link_args)", 0, policy)
    run = source.index("link_result = _run_completed_command(", policy)
    assert user_args < policy < run


def test_native_link_identity_and_fallback_policy_has_one_source_authority() -> None:
    cli_root = Path(__file__).resolve().parents[2] / "src" / "molt" / "cli"
    allowed_identity_authority = cli_root / "native_link_plan.py"
    authority_source = allowed_identity_authority.read_text(encoding="utf-8")
    assert "def native_link_policy_flags(" in authority_source
    assert "def native_dead_strip_identity_flags(" not in authority_source
    assert "def native_reproducible_link_flags(" not in authority_source
    identity_tokens = ("/Brepro", "/OPT:NOICF", "-no_deduplicate", "--icf=none")
    for source_path in cli_root.glob("*.py"):
        if source_path == allowed_identity_authority:
            continue
        source = source_path.read_text(encoding="utf-8")
        for token in identity_tokens:
            assert token not in source, (
                f"native link identity policy drifted into {source_path.name}: {token}"
            )

    main_link = inspect.getsource(link_pipeline._prepare_native_link)
    extension_link = inspect.getsource(commands.extension_build)
    extension_policy = inspect.getsource(
        __import__(
            "molt.cli.source_extensions",
            fromlist=["_source_extension_link_policy_args"],
        )._source_extension_link_policy_args
    )
    assert "_build_native_link_plan(" in main_link
    assert "_native_link_execution_command(" in main_link
    assert "_source_extension_link_policy_args(" in extension_link
    assert "native_link_policy_flags(" in extension_policy
    assert "_retry_native_link" not in main_link
    assert "Linker fallback:" not in main_link
