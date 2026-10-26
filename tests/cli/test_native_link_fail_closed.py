from __future__ import annotations

from molt.compilation_target import admit_target
from molt import native_target

import inspect
from pathlib import Path
import subprocess
import platform
import sys

import pytest

from molt.cli import backend_output_pipeline, extension_commands, native_toolchain
from molt.cli import build_results, link_pipeline, native_link_command
from tests.process_guard_common import install_module_view


def test_native_link_has_one_attempt_and_no_post_failure_fallback() -> None:
    source = inspect.getsource(link_pipeline._prepare_native_link)
    assert source.count("_run_native_link_command(") == 1
    assert "retry" not in source.lower()
    assert "Linker fallback:" not in source


def test_runtime_build_identity_is_verified_before_every_production_link() -> None:
    pipeline = inspect.getsource(backend_output_pipeline._emit_backend_pipeline_outputs)
    ensure = pipeline.index("_ensure_native_runtime_lib_ready_before_link(")
    failure = pipeline.index("return_after_build_diagnostics(", ensure)
    prepare = pipeline.index("_prepare_native_link(", failure)
    assert ensure < failure < prepare

    link = inspect.getsource(link_pipeline._prepare_native_link)
    verify = link.index("runtime_codegen_binding.verify()")
    identity = link.index(
        "runtime_build_identity=runtime_codegen_binding.build_identity"
    )
    run = link.index("_run_native_link_command(")
    assert verify < identity < run


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
    install_module_view(
        monkeypatch, "sys", sys, native_toolchain, native_target, platform="linux"
    )
    install_module_view(
        monkeypatch,
        "platform",
        platform,
        native_toolchain,
        native_target,
        machine=lambda: "x86_64",
    )
    monkeypatch.setattr(
        native_toolchain,
        "_run_completed_command",
        lambda *_args, **_kwargs: subprocess.CompletedProcess([], 0, "", ""),
    )
    assert (
        native_toolchain._run_bolt_post_link(
            bolt_requested=True,
            bolt_training_cmd=None,
            target=admit_target("native"),
            output=str(binary),
            out_dir=None,
            build_rc=0,
            json_output=True,
        )
        == 1
    )


@pytest.mark.parametrize("has_link_selection", [False, True])
def test_bolt_finalizes_candidate_before_atomic_publication(
    has_link_selection, tmp_path, monkeypatch
) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    (tools / "bolt_optimize.sh").write_text("#!/usr/bin/env bash\n", encoding="utf-8")
    binary = tmp_path / "program"
    bolt_binary = Path(f"{binary}.bolt")
    binary.write_bytes(b"original")
    bolt_binary.write_bytes(b"optimized")
    selection_candidate = tmp_path / "selection.candidate.json"
    selection_candidate.write_text("{}", encoding="utf-8")
    selection = (
        (selection_candidate, tmp_path / "selection.json")
        if has_link_selection
        else None
    )
    received: dict[str, object] = {}
    monkeypatch.setattr(native_toolchain, "_compiler_root", lambda: tmp_path)
    install_module_view(
        monkeypatch, "sys", sys, native_toolchain, native_target, platform="linux"
    )
    install_module_view(
        monkeypatch,
        "platform",
        platform,
        native_toolchain,
        native_target,
        machine=lambda: "x86_64",
    )
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
            target=admit_target("native"),
            output=str(binary),
            out_dir=None,
            build_rc=0,
            json_output=True,
            link_selection=selection,
        )
        == 0
    )
    assert received == {
        "candidate": bolt_binary,
        "output_binary": binary,
        "target_triple": None,
        "strip": True,
        "receipt": None,
        "link_selection": selection,
        "link_input_artifacts": None,
        "validate_inputs": None,
    }
    assert binary.read_bytes() == b"finalized"


@pytest.mark.parametrize("mutation", ["rewrite", "delete"])
@pytest.mark.parametrize("owned_input", [False, True])
def test_bolt_source_rejection_reclaims_admitted_private_candidates(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str, owned_input: bool
) -> None:
    from molt import file_publication
    from molt.cli import atomic_io
    from molt.cli.module_source import _ModuleSourceCatalog, _ModuleSourceLease
    from tests.native_artifact_fixtures import elf_header

    source = tmp_path / "app.py"
    source.write_text("value = 1\n", encoding="utf-8")
    catalog = _ModuleSourceCatalog({"app": _ModuleSourceLease.path_backed(source)})
    output = tmp_path / "program"
    output.write_bytes(b"previous public binary")
    original = (
        file_publication.staged_file_path(output, purpose="native-link", suffix=".tmp")
        if owned_input
        else output
    )
    if owned_input:
        original.write_bytes(b"private unoptimized binary")
    bolt_binary = Path(f"{original}.bolt")
    selection_output = tmp_path / "selection.json"
    selection_output.write_bytes(b"previous public selection")
    selection = file_publication.staged_file_path(
        selection_output, purpose="native-link"
    )
    selection.write_bytes(b"private selection")
    unrelated = tmp_path / "unrelated"
    unrelated.write_bytes(b"retained")
    tools = tmp_path / "tools"
    tools.mkdir()
    (tools / "bolt_optimize.sh").write_text("#!/usr/bin/env bash\n", encoding="utf-8")
    monkeypatch.setattr(native_toolchain, "_compiler_root", lambda: tmp_path)
    install_module_view(
        monkeypatch, "sys", sys, native_toolchain, native_target, platform="linux"
    )
    install_module_view(
        monkeypatch,
        "platform",
        platform,
        native_toolchain,
        native_target,
        machine=lambda: "x86_64",
    )
    monkeypatch.setenv("MOLT_KEEP_SYMBOLS", "1")
    monkeypatch.delenv("MOLT_SKIP_BINARY_VALIDITY_CHECK", raising=False)
    monkeypatch.delenv("MOLT_BUILD_SMOKE_EXEC", raising=False)
    staged: list[Path] = []

    def optimize(command, **kwargs):
        assert command[2] == str(original)
        bolt_binary.write_bytes(bytes(elf_header(kind=2)))
        return subprocess.CompletedProcess(command, 0, "", "")

    validate = build_results._assert_native_binary_valid

    def validate_then_mutate(stage: Path, target_triple: str | None) -> None:
        # Stage validation is the last step before the source fence.
        validate(stage, target_triple)
        staged.append(stage)
        assert output.read_bytes() == b"previous public binary"
        if mutation == "rewrite":
            source.write_text("value = 2\n", encoding="utf-8")
        else:
            source.unlink()

    def sign(stage: Path, destination: Path) -> None:
        raise AssertionError("BOLT writes ELF images, which carry no signature")

    monkeypatch.setattr(native_toolchain, "_run_completed_command", optimize)
    monkeypatch.setattr(
        build_results, "_assert_native_binary_valid", validate_then_mutate
    )
    monkeypatch.setattr(atomic_io, "_codesign_atomic_copy_temp", sign)
    assert (
        native_toolchain._run_bolt_post_link(
            bolt_requested=True,
            bolt_training_cmd=None,
            target=admit_target("x86_64-unknown-linux-gnu"),
            input_binary=str(original) if owned_input else None,
            output=str(output),
            out_dir=None,
            build_rc=0,
            json_output=True,
            link_selection=(selection, selection_output),
            validate_inputs=catalog.validate,
        )
        == 1
    )
    assert output.read_bytes() == b"previous public binary"
    assert selection_output.read_bytes() == b"previous public selection"
    assert not bolt_binary.exists() and not selection.exists()
    assert staged and all(not stage.exists() for stage in staged)
    assert not original.exists() if owned_input else original == output
    assert unrelated.read_bytes() == b"retained"


def test_bolt_helper_never_emits_json_outside_build_result_authority(
    tmp_path, monkeypatch, capsys
) -> None:
    binary = tmp_path / "program"
    binary.write_bytes(b"input")
    monkeypatch.setattr(native_toolchain, "_compiler_root", lambda: tmp_path)
    install_module_view(
        monkeypatch, "sys", sys, native_toolchain, native_target, platform="linux"
    )
    install_module_view(
        monkeypatch,
        "platform",
        platform,
        native_toolchain,
        native_target,
        machine=lambda: "x86_64",
    )

    assert (
        native_toolchain._run_bolt_post_link(
            bolt_requested=True,
            bolt_training_cmd=None,
            target=admit_target("native"),
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
    extension_link = inspect.getsource(extension_commands.extension_build)
    final_link = inspect.getsource(native_link_command._build_native_link_plan)
    assert "_build_native_link_plan(" in main_link
    assert "_native_link_execution_command(" in main_link
    assert "source_extension_link_requirements(" in extension_link
    assert '"rcsD"' in extension_link
    assert "native_link_policy_flags(" in final_link
    assert "_source_extension_link_policy_args" not in (
        cli_root / "source_extensions.py"
    ).read_text(encoding="utf-8")
    assert "_retry_native_link" not in main_link
    assert "Linker fallback:" not in main_link
