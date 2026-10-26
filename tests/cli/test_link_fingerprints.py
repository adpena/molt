from __future__ import annotations

from molt.compilation_target import admit_target

from molt.native_target import resolve_native_target_spec
from molt.native_target_shape import NativeObjectFormat

from tests.cli.native_link_test_support import native_codegen_binding
from tests.native_artifact_fixtures import signed_macho_image

from copy import deepcopy
import json
import os
from pathlib import Path, PurePath, PurePosixPath, PureWindowsPath
import subprocess

import pytest

from molt.capability_manifest import CapabilityManifest
from molt import artifact_publication
from molt.cli import atomic_io
from molt.cli import build_results, link_fingerprints as receipts, link_pipeline
from molt.cli import native_link_command
from molt.link_outputs import validate_link_output_paths, wasm_link_output_paths
from tests.cli.native_link_test_support import (
    write_test_native_link_manifest,
    write_test_static_archive,
)


@pytest.mark.parametrize("route", ["execute", "cached"])
@pytest.mark.parametrize("which", ["archive", "callable_symbols"])
def test_native_link_consumer_rejects_codegen_generation_replacement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, route: str, which: str
) -> None:
    import contextlib

    target = "x86_64-unknown-linux-gnu"
    monkeypatch.setattr(
        native_link_command,
        "_build_native_link_driver_command",
        lambda **kwargs: (["clang", "-target", target], None, target),
    )
    monkeypatch.setattr(link_pipeline, "native_link_cache_tool_facts", lambda _plan: [])
    monkeypatch.setattr(
        link_pipeline,
        "native_link_selection",
        lambda *_a, **_k: contextlib.nullcontext(None),
    )
    monkeypatch.setattr(
        link_pipeline,
        "_native_link_execution_command",
        lambda plan, **kwargs: contextlib.nullcontext(
            [
                str(kwargs["execution_output"])
                if item == str(kwargs["planned_output"])
                else item
                for item in plan.command
            ]
        ),
    )
    runtime = tmp_path / "runtime.a"
    app = tmp_path / "app.a"
    write_test_static_archive(runtime)
    write_test_static_archive(app)
    identity = write_test_native_link_manifest(runtime, target_triple=target)
    binding = native_codegen_binding(runtime, identity)
    selected_path = getattr(binding, which).path
    binary = tmp_path / "app"
    binary.write_bytes(b"previously-published")
    candidates = []

    def replace_generation():
        original = selected_path.stat()
        replacement = selected_path.with_name(selected_path.name + ".replacement")
        replacement.write_bytes(selected_path.read_bytes())
        replacement.replace(selected_path)
        os.utime(selected_path, ns=(original.st_atime_ns, original.st_mtime_ns))

    def cached(**kwargs):
        if route == "cached":
            replace_generation()
            return True
        return False

    def run(*, link_cmd, **kwargs):
        assert route == "execute"
        candidate = Path(link_cmd[link_cmd.index("-o") + 1])
        candidate.write_bytes(b"new-unpublished")
        candidates.append(candidate)
        replace_generation()
        return subprocess.CompletedProcess(link_cmd, 0, "", "")

    monkeypatch.setattr(receipts, "_link_outputs_match", cached)
    monkeypatch.setattr(link_pipeline, "_run_native_link_command", run)
    prepared, failure = link_pipeline._prepare_native_link(
        output_artifact=app,
        resolved_capability_policy=CapabilityManifest().resolve(),
        artifacts_root=tmp_path,
        json_output=True,
        output_binary=binary,
        runtime_codegen_binding=binding,
        target=resolve_native_target_spec(target),
        sysroot_path=None,
        profile="dev",
        project_root=tmp_path,
        diagnostics_enabled=False,
        phase_starts={},
        link_timeout=None,
        warnings=[],
    )
    assert prepared is None and failure == 2
    assert binary.read_bytes() == b"previously-published"
    assert all(not path.exists() for path in candidates)
    assert len(candidates) == (1 if route == "execute" else 0)


def test_native_stub_reuse_preserves_exact_generated_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    target = "x86_64-unknown-linux-gnu"
    runtime = tmp_path / "runtime.a"
    write_test_static_archive(runtime)
    identity = write_test_native_link_manifest(runtime, target_triple=target)
    binding = native_codegen_binding(runtime, identity)
    policy = CapabilityManifest().resolve()
    expected = link_pipeline._render_native_main_stub(
        resolved_capability_policy=policy,
        compiled_source_paths=(),
        runtime_module_roots=(),
    ).encode("utf-8")
    assert b"int main(" in expected
    stub = tmp_path / "main_stub.c"
    publish = atomic_io.file_publication.atomic_write_bytes
    writes = []

    def record(destination, data, **kwargs):
        writes.append((destination, data))
        publish(destination, data, **kwargs)

    monkeypatch.setattr(atomic_io.file_publication, "atomic_write_bytes", record)

    def prepare_stub():
        # The existing missing-output boundary stops before linking; the real
        # producer and publication path still run without any child process.
        prepared, failure = link_pipeline._prepare_native_link(
            output_artifact=tmp_path / "app.a",
            resolved_capability_policy=policy,
            artifacts_root=tmp_path,
            json_output=True,
            output_binary=None,
            runtime_codegen_binding=binding,
            target=resolve_native_target_spec(target),
            sysroot_path=None,
            profile="dev",
            project_root=tmp_path,
            diagnostics_enabled=False,
            phase_starts={},
            link_timeout=None,
            warnings=[],
        )
        assert prepared is None and failure == 2

    prepare_stub()
    assert writes == [(stub, expected)]
    prepare_stub()
    assert writes == [(stub, expected)]
    stub.write_bytes(expected.replace(b"\n", b"\r\n"))
    prepare_stub()
    assert writes == [(stub, expected), (stub, expected)]
    assert stub.read_bytes() == expected


def _publish_receipt(tmp_path: Path, outputs: dict[str, Path]):
    source = tmp_path / "source.o"
    source.write_bytes(b"source")
    fingerprint = receipts._link_fingerprint(
        project_root=tmp_path, inputs=[source], link_cmd=["link", "source.o"]
    )
    assert fingerprint is not None
    sidecar = tmp_path / "link.fingerprint"
    candidates = {}
    for role, final in outputs.items():
        candidate = artifact_publication.staged_output_path(final)
        candidate.write_bytes(final.read_bytes())
        candidates[role] = (candidate, final)
    receipts.publish_link_outputs(
        candidates,
        receipt=receipts.FinalLinkReceiptRequest.from_fingerprint(sidecar, fingerprint),
    )
    stored = receipts._read_link_fingerprint(sidecar)
    assert stored is not None
    assert receipts._link_outputs_match(
        outputs=outputs, fingerprint=fingerprint, receipt_path=sidecar
    )
    return source, fingerprint, sidecar, stored


@pytest.mark.parametrize("role", ["linked", "app", "runtime", "size_attestation"])
@pytest.mark.parametrize("damage", ["missing", "same-size-mtime", "path"])
def test_every_split_output_role_is_content_and_path_bound(
    tmp_path: Path, role: str, damage: str
) -> None:
    outputs = wasm_link_output_paths(
        tmp_path / "linked.wasm", split_output_dir=tmp_path
    )
    for path in outputs.values():
        path.write_bytes(
            b"\0asm\x01\0\0\0\0\x02\x01a" if path.suffix == ".wasm" else b"{}"
        )
    _, fingerprint, sidecar, _ = _publish_receipt(tmp_path, outputs)
    damaged = outputs[role]
    if damage == "missing":
        damaged.unlink()
    elif damage == "same-size-mtime":
        before = damaged.stat()
        data = damaged.read_bytes()
        damaged.write_bytes(data[:-1] + b"b")
        os.utime(damaged, ns=(before.st_atime_ns, before.st_mtime_ns))
    else:
        replacement = tmp_path / ("relocated-" + damaged.name)
        replacement.write_bytes(damaged.read_bytes())
        outputs[role] = replacement
    assert not receipts._link_outputs_match(
        outputs=outputs, fingerprint=fingerprint, receipt_path=sidecar
    )


def test_input_rebuild_metadata_does_not_replace_content_identity(
    tmp_path: Path,
) -> None:
    output = tmp_path / "app.exe"
    output.write_bytes(b"binary")
    outputs = {"binary": output}
    source, fingerprint, sidecar, stored = _publish_receipt(tmp_path, outputs)
    for path in (source, output):
        before = path.stat()
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns + 2_000_000_000))
    unchanged = receipts._link_fingerprint(
        project_root=tmp_path,
        inputs=[source],
        link_cmd=["link", "source.o"],
        stored_fingerprint=stored["fingerprint"],
    )
    assert unchanged is not None and unchanged["hash"] == fingerprint["hash"]
    assert receipts._link_outputs_match(
        outputs=outputs, fingerprint=unchanged, receipt_path=sidecar
    )
    before = source.stat()
    source.write_bytes(b"mutant")
    os.utime(source, ns=(before.st_atime_ns, before.st_mtime_ns))
    changed = receipts._link_fingerprint(
        project_root=tmp_path,
        inputs=[source],
        link_cmd=["link", "source.o"],
        stored_fingerprint=stored["fingerprint"],
    )
    assert not receipts._link_outputs_match(
        outputs=outputs, fingerprint=changed, receipt_path=sidecar
    )


@pytest.mark.parametrize(
    "damage", ["old", "unknown", "version", "numeric", "role", "truncated"]
)
def test_unusable_receipts_are_cache_misses(tmp_path: Path, damage: str) -> None:
    binary = tmp_path / "app.exe"
    binary.write_bytes(b"binary")
    outputs = {"binary": binary}
    _, fingerprint, sidecar, stored = _publish_receipt(tmp_path, outputs)
    payload = deepcopy(stored)
    if damage == "old":
        payload = payload["fingerprint"]
    elif damage == "unknown":
        payload["unowned"] = True
    elif damage == "version":
        payload["fingerprint"]["version"] = True
    elif damage == "numeric":
        payload["outputs"]["binary"]["identity"]["size_bytes"] = True
    elif damage == "role":
        payload["outputs"]["wrong"] = payload["outputs"].pop("binary")
    sidecar.write_text(
        "{" if damage == "truncated" else json.dumps(payload), encoding="utf-8"
    )
    assert not receipts._link_outputs_match(
        outputs=outputs,
        fingerprint=fingerprint,
        receipt_path=sidecar,
    )


@pytest.mark.parametrize(
    ("coordinate", "accepted"),
    [
        ("/work/output/program", True),
        (r"C:\work\output\program.exe", True),
        (r"\\server\share\output\program.exe", True),
        ("relative/program", False),
        ("/work/../output/program", False),
        ("/work//output/program", False),
        ("/work/./output/program", False),
        ("C:program.exe", False),
        (r"\work\output\program.exe", False),
        ("C:/work/output/program.exe", False),
        (r"\\?\C:\work\program.exe", False),
        ("/work/program\x00", False),
    ],
)
def test_retained_link_receipt_validates_the_producing_host_coordinate(
    tmp_path: Path, coordinate: str, accepted: bool
) -> None:
    binary = tmp_path / "program"
    binary.write_bytes(b"independent publication content")
    _, _, sidecar, payload = _publish_receipt(tmp_path, {"binary": binary})
    payload["outputs"]["binary"]["path"] = coordinate
    sidecar.write_text(json.dumps(payload), encoding="utf-8")
    assert receipts._read_link_fingerprint(sidecar) == (payload if accepted else None)


def test_link_output_aliases_are_rejected_before_publication(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="alias the same path"):
        wasm_link_output_paths(tmp_path / "app.wasm", split_output_dir=tmp_path)
    with pytest.raises(ValueError, match="aliases input"):
        wasm_link_output_paths(
            tmp_path / "linked.wasm",
            split_output_dir=tmp_path,
            inputs=[tmp_path / "nested" / ".." / "molt_runtime.wasm"],
        )
    with pytest.raises(ValueError, match="aliases input"):
        validate_link_output_paths(
            {"binary": tmp_path / "app"}, inputs=[tmp_path / "app"]
        )


def test_receipt_never_adopts_a_later_publishers_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output = tmp_path / "app"
    output.write_bytes(b"old")
    _, fingerprint, sidecar, _ = _publish_receipt(tmp_path, {"binary": output})
    request = receipts.FinalLinkReceiptRequest.from_fingerprint(sidecar, fingerprint)
    assert request is not None
    transport = tmp_path / "request.json"
    transport.write_bytes(request.encode())
    request = receipts.FinalLinkReceiptRequest.read(transport)
    candidate = artifact_publication.staged_output_path(output)
    candidate.write_bytes(b"producer A")
    real_publish = artifact_publication.publish_validated_outputs

    def racing_publish(pairs, **kwargs):
        real_publish(pairs, **kwargs)
        # Deterministic interleaving: another producer commits after A's
        # publication but before A returns to the caller. A must never mint
        # a receipt labeling B's bytes as its own input generation.
        rival = artifact_publication.staged_output_path(output)
        rival.write_bytes(b"producer B")
        real_publish([(rival, output)])

    monkeypatch.setattr(
        artifact_publication, "publish_validated_outputs", racing_publish
    )
    receipts.publish_link_outputs({"binary": (candidate, output)}, receipt=request)
    stored = receipts._read_link_fingerprint(sidecar)
    assert stored is not None and output.read_bytes() == b"producer B"
    assert not receipts._link_outputs_match(
        outputs={"binary": output}, fingerprint=fingerprint, receipt_path=sidecar
    )


def test_observation_only_receipts_are_not_generation_evidence(tmp_path: Path) -> None:
    output = tmp_path / "app"
    output.write_bytes(b"old")
    _, _, sidecar, stored = _publish_receipt(tmp_path, {"binary": output})
    stored["schema"] = "molt.final-link.v1"
    sidecar.write_text(json.dumps(stored), encoding="utf-8")
    assert receipts._read_link_fingerprint(sidecar) is None


def test_archive_candidate_identity_uses_final_role_not_private_suffix(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "libapp.a"
    write_test_static_archive(archive)
    _, fingerprint, sidecar, _ = _publish_receipt(tmp_path, {"archive": archive})
    assert receipts._link_outputs_match(
        outputs={"archive": archive}, fingerprint=fingerprint, receipt_path=sidecar
    )


def test_obsolete_receipt_cannot_authorize_retirement_or_block_rebuild(
    tmp_path: Path,
) -> None:
    output = tmp_path / "app.bin"
    output.write_bytes(b"old")
    unrelated = tmp_path / "unrelated.bin"
    unrelated.write_bytes(b"preserve")
    _, fingerprint, sidecar, old = _publish_receipt(tmp_path, {"binary": output})
    old["schema"] = "molt.final-link.v1"
    old["outputs"]["untrusted"] = {
        "path": str(unrelated),
        "identity": receipts.artifact_content_identity(unrelated),
    }
    sidecar.write_text(json.dumps(old), encoding="utf-8")
    stage = artifact_publication.staged_output_path(output)
    stage.write_bytes(b"new")
    receipts.publish_link_outputs(
        {"binary": (stage, output)},
        receipt=receipts.FinalLinkReceiptRequest.from_fingerprint(sidecar, fingerprint),
        retire_previous_outputs_under=tmp_path,
    )
    assert output.read_bytes() == b"new" and unrelated.read_bytes() == b"preserve"
    assert receipts._link_outputs_match(
        outputs={"binary": output}, fingerprint=fingerprint, receipt_path=sidecar
    )


@pytest.mark.parametrize("state", ["unchanged", "changed", "absent"])
def test_generation_retires_only_unchanged_previous_family_members(
    tmp_path: Path, state: str
) -> None:
    output = tmp_path / "app.wasm"
    old_module = b"\0asm\x01\0\0\0\0\x02\x01a"
    new_module = b"\0asm\x01\0\0\0\0\x02\x01b"
    output.write_bytes(old_module)
    obsolete = tmp_path / "assets" / "retired.js"
    obsolete.parent.mkdir()
    obsolete.write_bytes(b"old asset")
    _, fingerprint, sidecar, _ = _publish_receipt(
        tmp_path, {"linked": output, "asset": obsolete}
    )
    unrelated = obsolete.parent / "keep.js"
    unrelated.write_bytes(b"unrelated")
    candidate = artifact_publication.staged_output_path(output)
    candidate.write_bytes(new_module)

    def publish():
        receipts.publish_link_outputs(
            {"linked": (candidate, output)},
            receipt=receipts.FinalLinkReceiptRequest.from_fingerprint(
                sidecar, fingerprint
            ),
            retire_previous_outputs_under=tmp_path,
        )

    if state == "changed":
        obsolete.write_bytes(b"user changed asset")
        with pytest.raises(ValueError, match="changed outside publication"):
            publish()
        assert output.read_bytes() == old_module
        assert obsolete.read_bytes() == b"user changed asset"
    else:
        if state == "absent":
            obsolete.unlink()
        publish()
        assert not obsolete.exists()
        assert output.read_bytes() == new_module
        assert receipts._link_outputs_match(
            outputs={"linked": output}, fingerprint=fingerprint, receipt_path=sidecar
        )
    assert unrelated.read_bytes() == b"unrelated"


def test_family_receipt_follows_destination_not_project_cache_policy(
    tmp_path, monkeypatch
):
    output = tmp_path / "out" / "app"
    first = receipts._link_fingerprint_path(output)
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(tmp_path / "different-cache"))
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "different-runtime-cache"))
    assert receipts._link_fingerprint_path(output) == first
    assert first.parent == output.parent


@pytest.mark.parametrize("damage", ["relative", "schema", "extra", "fingerprint"])
def test_invalid_receipt_transport_is_rejected(tmp_path: Path, damage: str) -> None:
    output = tmp_path / "app"
    output.write_bytes(b"old")
    _, fingerprint, sidecar, _ = _publish_receipt(tmp_path, {"binary": output})
    request = receipts.FinalLinkReceiptRequest.from_fingerprint(sidecar, fingerprint)
    assert request is not None
    payload = json.loads(request.encode())
    if damage == "relative":
        payload["path"] = "relative-receipt"
    elif damage == "schema":
        payload["schema"] = "unowned"
    elif damage == "extra":
        payload["unowned"] = True
    else:
        payload["fingerprint"]["version"] = True
    transport = tmp_path / "request.json"
    transport.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError):
        receipts.FinalLinkReceiptRequest.read(transport)


@pytest.mark.parametrize(
    "target",
    [
        "x86_64-pc-windows-msvc",
        "x86_64-pc-windows-gnu",
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
    ],
)
def test_native_consumer_reuses_published_bytes_and_relinks_tampering(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, target: str
) -> None:
    monkeypatch.setattr(
        native_link_command,
        "_build_native_link_driver_command",
        lambda **kwargs: (["clang", "-target", target], None, target),
    )
    monkeypatch.setattr(link_pipeline, "native_link_cache_tool_facts", lambda plan: [])
    # This test owns caching/publication, not compiler execution or binary-format
    # validation. Leave real planning, fingerprints, receipt IO and reuse intact.
    monkeypatch.setattr(
        build_results, "_assert_native_binary_valid", lambda *args: None
    )
    monkeypatch.setattr(
        link_pipeline, "_darwin_link_validation_failure", lambda **kwargs: None
    )
    linked: list[Path] = []
    darwin = (
        resolve_native_target_spec(target).object_format is NativeObjectFormat.MACHO
    )

    def run(*, link_cmd, **kwargs):
        candidate = Path(link_cmd[link_cmd.index("-o") + 1])
        if darwin:
            # Apple ld signs the stage under its -final_output name.
            named = Path(link_cmd[link_cmd.index("-final_output") + 2])
            candidate.write_bytes(bytes(signed_macho_image(named.name)))
        else:
            candidate.write_bytes(b"raw binary")
        linked.append(candidate)
        return subprocess.CompletedProcess(link_cmd, 0, "", "")

    def sign(candidate, destination):
        raise AssertionError("no linked image here needs a post-link signature")

    monkeypatch.setattr(link_pipeline, "_run_native_link_command", run)
    monkeypatch.setattr(atomic_io, "_codesign_atomic_copy_temp", sign)
    runtime = tmp_path / "runtime.a"
    app = tmp_path / "app.a"
    for path in (runtime, app):
        write_test_static_archive(path)
    identity = write_test_native_link_manifest(runtime, target_triple=target)
    binary = tmp_path / "app.exe"
    policy = CapabilityManifest().resolve()

    def prepare():
        phases: dict[str, float] = {}
        result, failure = link_pipeline._prepare_native_link(
            output_artifact=app,
            resolved_capability_policy=policy,
            artifacts_root=tmp_path,
            json_output=True,
            output_binary=binary,
            runtime_codegen_binding=native_codegen_binding(runtime, identity),
            target=resolve_native_target_spec(target),
            sysroot_path=None,
            profile="dev",
            project_root=tmp_path,
            diagnostics_enabled=True,
            phase_starts=phases,
            link_timeout=None,
            warnings=[],
        )
        assert failure is None and result is not None
        assert "link" in phases
        return result

    first = prepare()
    assert not first.link_skipped and len(linked) == 1
    warnings: list[str] = []
    status = build_results._emit_native_link_result(
        link_process=first.link_process,
        link_skipped=False,
        link_fingerprint=first.link_fingerprint,
        link_fingerprint_path=first.link_fingerprint_path,
        link_candidate=first.link_output,
        output_binary=binary,
        cache=True,
        cache_hit=False,
        cache_key=None,
        function_cache_key=None,
        cache_path=None,
        function_cache_path=None,
        cache_hit_tier=None,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_config_digest=None,
        target=admit_target("native"),
        target_triple=target,
        source_path=tmp_path / "app.py",
        deterministic=True,
        trusted=False,
        resolved_capability_policy=policy,
        capabilities_source=None,
        sysroot_path=None,
        emit_mode="binary",
        profile="dev",
        native_arch_perf_enabled=False,
        output_obj=app,
        stub_path=first.stub_path,
        runtime_lib=runtime,
        external_native_artifacts=(),
        diagnostics_enabled=False,
        build_diagnostics_payload=lambda: (None, None),
        pgo_profile_payload=None,
        runtime_feedback_payload=None,
        emit_ir_path=None,
        stdlib_obj_path=None,
        warnings=warnings,
        json_output=True,
        resolved_diagnostics_verbosity="brief",
        strip_after_link=False,
    )
    assert status == 0 and not warnings
    assert binary.read_bytes() == (
        bytes(signed_macho_image(binary.name)) if darwin else b"raw binary"
    )
    assert prepare().link_skipped and len(linked) == 1
    before = app.stat()
    os.utime(app, ns=(before.st_atime_ns, before.st_mtime_ns + 2_000_000_000))
    assert prepare().link_skipped and len(linked) == 1
    before = binary.stat()
    binary.write_bytes(b"bad binary signed")
    os.utime(binary, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert not prepare().link_skipped and len(linked) == 2


def _fixture_lld_archive_coordinate(path: PurePath) -> str:
    # Independent LLD relativeToRoot fixture: keep drive/server components,
    # omit the host root marker and use archive slashes. Never call Molt's
    # projection to manufacture the reproducer that it must validate.
    spelling = path.as_posix()
    if isinstance(path, PureWindowsPath) and path.drive.endswith(":"):
        spelling = spelling.replace(":", "", 1)
    return spelling.lstrip("/")


def _fixture_make_dependency(path: PurePath) -> str:
    # Emit a Make prerequisite, including native Windows separators. Dollar
    # doubling is Make syntax; this control exercises space/#/Unicode paths.
    return "".join(
        "$$"
        if character == "$"
        else "\\" + character
        if character in "\\ #:"
        else character
        for character in str(path)
    )


@pytest.mark.parametrize(
    ("path", "archive_name", "make_name"),
    [
        (
            PurePosixPath("/work/link inputs/λ#1.o"),
            "work/link inputs/λ#1.o",
            "/work/link\\ inputs/λ\\#1.o",
        ),
        (
            PureWindowsPath("C:/work/link inputs/λ#1.o"),
            "C/work/link inputs/λ#1.o",
            "C\\:\\\\work\\\\link\\ inputs\\\\λ\\#1.o",
        ),
        (
            PureWindowsPath("//server/share/link inputs/λ#1.o"),
            "server/share/link inputs/λ#1.o",
            "\\\\\\\\server\\\\share\\\\link\\ inputs\\\\λ\\#1.o",
        ),
    ],
)
def test_native_link_fixture_uses_literal_lld_and_make_coordinates(
    path, archive_name, make_name
):
    assert _fixture_lld_archive_coordinate(path) == archive_name
    assert _fixture_make_dependency(path) == make_name


@pytest.mark.parametrize(
    "mutation",
    [None, "runtime", "compiler_output", "omitted", "duplicate", "missing_origin"],
)
def test_admission_link_capture_checks_actual_buffers_before_publication(
    tmp_path, monkeypatch, mutation, capsys
):
    import hashlib
    import io
    import tarfile
    from molt.backend_protocol import BackendEmissionOrigin
    from molt.cli.link_input_observation import LinkInputObservation

    target = "x86_64-unknown-linux-gnu"
    runtime, app, binary = (tmp_path / name for name in ("runtime.a", "app.a", "app"))
    write_test_static_archive(runtime)
    write_test_static_archive(app)
    admitted_runtime, admitted_app = runtime.read_bytes(), app.read_bytes()
    identity = write_test_native_link_manifest(runtime, target_triple=target)
    binding = native_codegen_binding(runtime, identity)
    origin = BackendEmissionOrigin.from_payload(
        {
            "schema": "molt.backend-emission-origin.v1",
            "producer": {"kind": "daemon", "process_id": 123, "scope": None},
            "input": {
                "schema": "molt.addressed-program-input.v1",
                "lease": {"size": 3, "sha256": hashlib.sha256(b"IR!").hexdigest()},
                "function_digests": "",
            },
            "output": {
                "size": len(admitted_app),
                "sha256": hashlib.sha256(admitted_app).hexdigest(),
            },
            "codegen": {"kind": "in-process", "process_id": 123},
        }
    )
    monkeypatch.setattr(
        native_link_command,
        "_build_native_link_driver_command",
        lambda **_: (["clang", "-target", target], "lld", target),
    )
    monkeypatch.setattr(link_pipeline, "native_link_cache_tool_facts", lambda _: [])
    driver = tmp_path / "clang"
    driver.write_bytes(b"fixture selected driver")
    header = tmp_path / "stdio λ #1.h"
    header.write_bytes(b"fixture standard header")
    linker = tmp_path / "ld.lld"
    linker.write_bytes(b"fixture selected linker")
    monkeypatch.setattr(
        link_pipeline,
        "native_link_tool_facts",
        lambda _: [
            {"role": "linker", "resolved": True, "path": str(linker)},
            {"role": "driver", "resolved": True, "path": str(driver)},
        ],
    )
    # An existing final-link hit must not fabricate this session's consumed bytes.
    monkeypatch.setattr(receipts, "_link_outputs_match", lambda **_: True)
    calls, stages = [], []

    def run(*, link_cmd, **_):
        calls.append(link_cmd)
        report = Path(
            next(
                arg.split("=", 1)[1]
                for arg in link_cmd
                if arg.startswith("--reproduce=")
            )
        )
        output = Path(link_cmd[link_cmd.index("-o") + 1])
        dependencies = Path(link_cmd[link_cmd.index("-MF") + 1])
        stub = tmp_path / "main_stub.c"
        dependencies.write_text(
            "output: "
            + " ".join(_fixture_make_dependency(path) for path in (stub, header))
            + "\n",
            encoding="utf-8",
        )
        stages.extend((report, output, dependencies))
        output.write_bytes(b"new-private-output")
        with tarfile.open(report, "w", format=tarfile.GNU_FORMAT) as archive:
            values = [
                ("response.txt", b"fixture command"),
                ("version.txt", b"fixture linker"),
            ]
            for role, path, data in (
                ("runtime", runtime, admitted_runtime),
                ("compiler_output", app, admitted_app),
            ):
                if mutation == "omitted" and role == "runtime":
                    continue
                values.append(
                    (
                        _fixture_lld_archive_coordinate(path),
                        b"X" * len(data) if mutation == role else data,
                    )
                )
            if mutation == "duplicate":
                values.append(values[-1])
            for name, data in values:
                member = tarfile.TarInfo(report.stem + "/" + name)
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data))
        return subprocess.CompletedProcess(link_cmd, 0, "", "")

    monkeypatch.setattr(link_pipeline, "_run_native_link_command", run)
    prepared, failure = link_pipeline._prepare_native_link(
        output_artifact=app,
        resolved_capability_policy=CapabilityManifest().resolve(),
        artifacts_root=tmp_path,
        json_output=True,
        output_binary=binary,
        runtime_codegen_binding=binding,
        target=resolve_native_target_spec(target),
        sysroot_path=None,
        profile="dev",
        project_root=tmp_path,
        diagnostics_enabled=False,
        phase_starts={},
        link_timeout=None,
        warnings=[],
        capture_link_inputs=True,
        emission_origin=None if mutation == "missing_origin" else origin,
    )
    assert len(calls) == (0 if mutation == "missing_origin" else 1)
    assert not binary.exists()
    if mutation is not None:
        assert prepared is None and failure == 2
        assert all(not stage.exists() for stage in stages)
        diagnostic = capsys.readouterr()
        expected_error = {
            "runtime": "did not consume the admitted runtime bytes",
            "compiler_output": "did not consume the admitted compiler_output bytes",
            "omitted": "did not consume the admitted runtime bytes",
            "duplicate": "repeats or exceeds its input inventory",
            "missing_origin": "requires ELF LLD and retained compiler emission origin",
        }[mutation]
        assert expected_error in diagnostic.out + diagnostic.err
    else:
        assert failure is None and prepared is not None and not prepared.link_skipped
        observed = LinkInputObservation.from_record(
            prepared.link_input_observation["link"]
        )
        actual = {path: (size, digest) for path, size, digest in observed.inputs}
        assert actual[_fixture_lld_archive_coordinate(runtime)] == (
            len(admitted_runtime),
            hashlib.sha256(admitted_runtime).hexdigest(),
        )
        assert actual[_fixture_lld_archive_coordinate(app)] == (
            len(admitted_app),
            hashlib.sha256(admitted_app).hexdigest(),
        )
        assert set(prepared.link_input_artifacts) == {
            "input_observation",
            "input_reproducer",
            "driver_dependencies",
            "driver_preparation",
        }
        preparation = json.loads(
            prepared.link_input_artifacts["driver_preparation"][0].read_text(
                encoding="utf-8"
            )
        )
        assert (
            preparation["driver"]["sha256"]
            == hashlib.sha256(driver.read_bytes()).hexdigest()
        )
        assert (
            next(row for row in preparation["files"] if row["path"] == str(header))[
                "sha256"
            ]
            == hashlib.sha256(header.read_bytes()).hexdigest()
        )
        for stage, final in prepared.link_input_artifacts.values():
            assert stage.is_file() and not final.exists()


@pytest.mark.parametrize(
    "damage", [None, "compiler", "runtime", "extra_role", "extra_field"]
)
def test_wasm_compilation_observation_retains_typed_runtime_without_verification(
    tmp_path, damage
):
    import hashlib
    from molt.cli.link_input_observation import (
        compilation_input_observation,
        validate_compilation_input_observation,
    )

    def content(data):
        return {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}

    compiler = content(b"original compiler output")
    runtime = content(b"runtime module")
    origin = {
        "schema": "molt.backend-emission-origin.v1",
        "producer": {"kind": "daemon", "process_id": 123, "scope": None},
        "input": {
            "schema": "molt.addressed-program-input.v1",
            "lease": content(b"partial lease"),
            "function_digests": "",
        },
        "output": compiler,
        "codegen": {"kind": "in-process", "process_id": 123},
    }
    link = {
        "schema": "molt.wasm-link-inputs.v2",
        "source_inputs": {
            "compiler_output": compiler.copy(),
            "runtime": runtime.copy(),
            "deployment_runtime": None,
        },
        "links": {
            "linked": {
                "schema": "molt.link-input-observation.v2",
                "dialect": "wasm",
                "linker_path": "/wasm-ld",
                "inputs": [
                    {"archive_path": "app.wasm", **content(b"rewritten app")},
                    {"archive_path": "runtime.wasm", **runtime},
                ],
                "roles": {
                    "compiler_rewritten": "app.wasm",
                    "runtime": "runtime.wasm",
                },
                "reproducer": content(b"actual reproducer"),
            }
        },
    }
    if damage in {"compiler", "runtime"}:
        link["source_inputs"][
            "compiler_output" if damage == "compiler" else "runtime"
        ] = content(b"substitution")
    elif damage == "extra_role":
        link["links"]["linked"]["roles"]["invented"] = "app.wasm"
    elif damage == "extra_field":
        link["source_inputs"]["verified"] = True
    if damage:
        with pytest.raises(ValueError):
            compilation_input_observation(
                origin, link, stdlib=None, cache={"compiler": None, "stdlib": None}
            )
        return
    admitted = compilation_input_observation(
        origin, link, stdlib=None, cache={"compiler": None, "stdlib": None}
    )
    assert (
        validate_compilation_input_observation(json.loads(json.dumps(admitted)))
        == admitted
    )
    # Admission owns a validated snapshot, not aliases into the untrusted record.
    link["source_inputs"]["runtime"]["size"] = 999
    output = tmp_path / "app.wasm"
    output.write_bytes(b"linked app")
    facts = build_results._observed_build_toolchain(
        backend_bin=None,
        runtime_lib=None,
        output=output,
        link_input_observation=admitted,
    )
    assert facts["runtime"] == {
        "identity": runtime,
        "observation": "linker-input-buffer",
    }
    assert facts["compiled_with_verified"] is False
    assert facts["compiler"] is None


@pytest.mark.parametrize(
    ("host", "archive"),
    [
        ("/home/user/object.o", "home/user/object.o"),
        (r"/work/colon:back\slash.o", r"work/colon:back\slash.o"),
        ("/C:/literal.o", "C:/literal.o"),
        (r"C:\work\object.o", "C/work/object.o"),
        (r"d:\with space\café.o", "d/with space/café.o"),
        (r"\\server\share\objects\one.o", "server/share/objects/one.o"),
    ],
)
def test_lld_archive_coordinate_matches_upstream_literals(host, archive):
    from pathlib import PurePosixPath, PureWindowsPath
    from molt.cli.link_input_observation import lld_archive_path

    assert lld_archive_path(host) == archive
    pure = PurePosixPath(host) if host.startswith("/") else PureWindowsPath(host)
    assert lld_archive_path(pure) == archive


@pytest.mark.parametrize(
    "host",
    [
        "relative.o",
        "C:relative.o",
        r"\root-relative.o",
        "C:/work/object.o",
        r"\\?\C:\work\object.o",
        r"C:\work\..\object.o",
        "/work/../object.o",
        "/work//object.o",
        "/work/./object.o",
        "/bad\x00name",
        r"C:\work\NUL",
    ],
)
def test_lld_archive_projection_rejects_noncanonical_host_coordinates(host):
    from molt.cli.link_input_observation import lld_archive_path

    with pytest.raises(ValueError):
        lld_archive_path(host)


def _coordinate_capture_content(data):
    import hashlib

    return {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def _coordinate_capture_record(linker=r"C:\tools\ld.lld.exe"):
    return {
        "schema": "molt.link-input-observation.v2",
        "dialect": "elf-gnu",
        "linker_path": linker,
        "inputs": [
            {"archive_path": "C/work/object.a", **_coordinate_capture_content(b"IR!")},
            {
                "archive_path": "D/runtime/runtime.a",
                **_coordinate_capture_content(b"RT!"),
            },
        ],
        "roles": {
            "compiler_output": "C/work/object.a",
            "runtime": "D/runtime/runtime.a",
        },
        "reproducer": _coordinate_capture_content(b"fixture archive identity"),
    }


def _coordinate_reproducer(path, inputs, *, administrative=True):
    import io
    import tarfile

    with tarfile.open(path, "w", format=tarfile.GNU_FORMAT) as archive:
        entries = [("response.txt", b"command"), ("version.txt", b"LLD")]
        if not administrative:
            entries.pop()
        for name, data in [*entries, *inputs]:
            member = tarfile.TarInfo("repro/" + name)
            member.size = len(data)
            archive.addfile(member, io.BytesIO(data))


@pytest.mark.parametrize(
    "linker", ["/tools/ld.lld", r"C:\tools\ld.lld.exe", r"\\server\share\ld.lld.exe"]
)
def test_link_capture_keeps_foreign_host_and_archive_coordinates_distinct(
    tmp_path, linker
):
    import io
    from contextlib import contextmanager
    from molt.cli.link_input_observation import (
        capture_link_inputs,
        LinkInputObservation,
    )

    retained = {
        "C/work/object.a": b"IR!",
        "D/runtime/runtime.a": b"RT!",
        "extra/unknown.o": b"other retained bytes",
        r"literal/colon:back\slash.o": b"POSIX filename bytes",
    }
    report = tmp_path / "capture.tar"
    _coordinate_reproducer(report, list(retained.items()))
    sinks, streams = {}, []

    @contextmanager
    def sink(coordinate, size):
        assert coordinate in retained
        assert size == len(retained[coordinate])
        with io.BytesIO() as stream:
            streams.append(stream)
            yield stream
            sinks[coordinate] = stream.getvalue()

    observed = capture_link_inputs(
        report,
        dialect="elf-gnu",
        linker_path=linker,
        expected={
            "compiler_output": ("C/work/object.a", _coordinate_capture_content(b"IR!")),
            "runtime": ("D/runtime/runtime.a", _coordinate_capture_content(b"RT!")),
        },
        input_sink=sink,
    )
    assert observed.linker_path == linker
    assert sinks == retained and all(stream.closed for stream in streams)
    assert dict(observed.roles) == {
        "compiler_output": "C/work/object.a",
        "runtime": "D/runtime/runtime.a",
    }
    assert observed.inputs == tuple(
        (name, len(data), _coordinate_capture_content(data)["sha256"])
        for name, data in sorted(retained.items())
    )
    assert LinkInputObservation.from_record(observed.as_record()) == observed
    assert set(observed.as_record()["inputs"][0]) == {"archive_path", "size", "sha256"}
    assert sorted(path.name for path in tmp_path.iterdir()) == ["capture.tar"]


@pytest.mark.parametrize(
    "coordinate",
    [
        "",
        ".",
        "..",
        "/absolute/input",
        "a/../input",
        "a/./input",
        "a//input",
        "a/input/",
        "bad\x00input",
    ],
)
def test_link_observation_rejects_malformed_archive_coordinates_before_read(
    tmp_path, monkeypatch, coordinate
):
    from molt.cli import link_input_observation as authority

    record = _coordinate_capture_record()
    record["inputs"][0]["archive_path"] = coordinate
    with pytest.raises(ValueError, match="archive coordinate"):
        authority.LinkInputObservation.from_record(record)

    def forbidden(*_args, **_kwargs):
        pytest.fail("invalid expected coordinate must fail before archive I/O")

    monkeypatch.setattr(authority, "open_stable_regular_file", forbidden)
    with pytest.raises(ValueError, match="archive coordinate"):
        authority.capture_link_inputs(
            tmp_path / "absent.tar",
            dialect="elf-gnu",
            linker_path=r"C:\tools\ld.lld.exe",
            expected={
                "compiler_output": (coordinate, _coordinate_capture_content(b"IR!"))
            },
        )


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        ("content", "admitted compiler_output bytes"),
        ("omitted", "admitted runtime bytes"),
        ("duplicate", "repeats"),
        ("traversal", "archive coordinate"),
        ("nonregular", "noncanonical input"),
        ("administrative", "omitted its command or version"),
    ],
)
def test_link_capture_rejects_actual_buffer_and_archive_mutants(
    tmp_path, mutation, message
):
    from molt.cli.link_input_observation import capture_link_inputs

    entries = [("C/work/object.a", b"IR!"), ("D/runtime/runtime.a", b"RT!")]
    if mutation == "content":
        entries[0] = (entries[0][0], b"BAD")
    elif mutation == "omitted":
        entries.pop()
    elif mutation == "duplicate":
        entries.append(entries[-1])
    elif mutation == "traversal":
        entries.append(("a/../bad", b"bad"))
    report = tmp_path / "capture.tar"
    _coordinate_reproducer(report, entries, administrative=mutation != "administrative")
    if mutation == "nonregular":
        import tarfile

        with tarfile.open(report, "a", format=tarfile.GNU_FORMAT) as archive:
            member = tarfile.TarInfo("repro/link")
            member.type = tarfile.SYMTYPE
            member.linkname = "../../outside"
            archive.addfile(member)
    before = report.read_bytes()
    with pytest.raises(ValueError, match=message):
        capture_link_inputs(
            report,
            dialect="elf-gnu",
            linker_path=r"C:\tools\ld.lld.exe",
            expected={
                "compiler_output": (
                    "C/work/object.a",
                    _coordinate_capture_content(b"IR!"),
                ),
                "runtime": ("D/runtime/runtime.a", _coordinate_capture_content(b"RT!")),
            },
        )
    assert report.read_bytes() == before
    assert sorted(path.name for path in tmp_path.iterdir()) == ["capture.tar"]


def _coordinate_preparation_record():
    return {
        "schema": "molt.native-link-preparation.v2",
        "driver": {
            "path": r"C:\tools\clang.exe",
            "content_path": r"C:\tools\clang.exe",
            **_coordinate_capture_content(b"driver"),
        },
        "source": r"D:\work\main.c",
        "files": [
            {
                "path": r"D:\work\main.c",
                "content_path": r"D:\work\main.c",
                **_coordinate_capture_content(b"source"),
            }
        ],
    }


def test_native_preparation_record_accepts_foreign_canonical_host_wire():
    from molt.cli.link_input_observation import validate_native_link_preparation

    record = _coordinate_preparation_record()
    assert validate_native_link_preparation(record) == record


@pytest.mark.parametrize(
    "host",
    [
        "C:/tools/clang.exe",
        "C:clang.exe",
        r"\rooted.exe",
        r"\\?\C:\tools\clang.exe",
        "/tools/../clang",
    ],
)
def test_native_preparation_and_linker_reject_host_aliases(host):
    from molt.cli.link_input_observation import (
        validate_native_link_preparation,
        LinkInputObservation,
    )

    preparation = _coordinate_preparation_record()
    preparation["driver"]["path"] = host
    with pytest.raises(ValueError):
        validate_native_link_preparation(preparation)
    with pytest.raises(ValueError):
        LinkInputObservation.from_record(_coordinate_capture_record(host))


def _coordinate_emission_origin():
    return {
        "schema": "molt.backend-emission-origin.v1",
        "producer": {"kind": "daemon", "process_id": 123, "scope": None},
        "input": {
            "schema": "molt.addressed-program-input.v1",
            "lease": _coordinate_capture_content(b"input"),
            "function_digests": "",
        },
        "output": _coordinate_capture_content(b"IR!"),
        "codegen": {"kind": "in-process", "process_id": 123},
    }


def test_compilation_coordinate_wrapper_uses_host_neutral_cache_paths():
    from molt.cli.link_input_observation import (
        compilation_input_observation,
        validate_compilation_input_observation,
    )

    cache = {"compiler": r"C:\cache\compiler.a", "stdlib": None}
    result = compilation_input_observation(
        _coordinate_emission_origin(),
        _coordinate_capture_record(),
        stdlib=None,
        cache=cache,
    )
    assert result["schema"] == "molt.compilation-inputs.v3"
    assert result["cache"] == cache
    assert validate_compilation_input_observation(result) == result
    for invalid in ["C:/cache/compiler.a", "C:compiler.a", "/cache/../compiler.a"]:
        with pytest.raises(ValueError):
            compilation_input_observation(
                _coordinate_emission_origin(),
                _coordinate_capture_record(),
                stdlib=None,
                cache={"compiler": invalid, "stdlib": None},
            )
    result["schema"] = "molt.compilation-inputs.v2"
    with pytest.raises(ValueError, match="contract is invalid"):
        validate_compilation_input_observation(result)


def test_link_coordinate_schemas_reject_obsolete_records():
    from molt.cli.link_input_observation import (
        LinkInputObservation,
        validate_native_link_preparation,
        validate_wasm_link_inputs,
    )

    observation = _coordinate_capture_record()
    observation["schema"] = "molt.link-input-observation.v1"
    with pytest.raises(ValueError, match="contract is invalid"):
        LinkInputObservation.from_record(observation)
    observation = _coordinate_capture_record()
    observation["inputs"][0]["path"] = observation["inputs"][0].pop("archive_path")
    with pytest.raises(ValueError, match="row is invalid"):
        LinkInputObservation.from_record(observation)
    preparation = _coordinate_preparation_record()
    preparation["schema"] = "molt.native-link-preparation.v1"
    with pytest.raises(ValueError, match="not exact"):
        validate_native_link_preparation(preparation)
    with pytest.raises(ValueError, match="contract is invalid"):
        validate_wasm_link_inputs(
            {"schema": "molt.wasm-link-inputs.v1", "source_inputs": {}, "links": {}}
        )


def test_link_capture_still_fences_retained_archive_mutation(tmp_path):
    from contextlib import contextmanager
    import os
    from molt.cli.link_input_observation import capture_link_inputs

    report = tmp_path / "capture.tar"
    _coordinate_reproducer(report, [("C/work/object.a", b"IR!")])
    before = report.stat()
    attempts = []

    @contextmanager
    def mutate_archive(_coordinate, _size):
        yield None
        attempts.append("write")
        # Change padding after the consumed members without changing the extent.
        # Windows may reject the write while its stable descriptor is retained;
        # either that denial or the real closing hash/fence must reject capture.
        with report.open("r+b") as stream:
            stream.seek(-1, 2)
            stream.write(b"X")
        os.utime(report, ns=(before.st_atime_ns, before.st_mtime_ns))

    with pytest.raises((OSError, ValueError)):
        capture_link_inputs(
            report,
            dialect="elf-gnu",
            linker_path=r"C:\tools\ld.lld.exe",
            expected={
                "compiler_output": (
                    "C/work/object.a",
                    _coordinate_capture_content(b"IR!"),
                )
            },
            input_sink=mutate_archive,
        )
    assert attempts == ["write"]
