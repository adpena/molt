from __future__ import annotations

from copy import deepcopy
import json
import os
from pathlib import Path
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

    def run(*, link_cmd, **kwargs):
        candidate = Path(link_cmd[link_cmd.index("-o") + 1])
        candidate.write_bytes(b"raw binary")
        linked.append(candidate)
        return subprocess.CompletedProcess(link_cmd, 0, "", "")

    def sign(candidate):
        candidate.write_bytes(candidate.read_bytes() + b" signed")

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
        result, failure = link_pipeline._prepare_native_link(
            output_artifact=app,
            resolved_capability_policy=policy,
            artifacts_root=tmp_path,
            json_output=True,
            output_binary=binary,
            runtime_lib=runtime,
            runtime_build_identity=identity,
            molt_root=tmp_path,
            runtime_cargo_profile="dev-fast",
            target_triple=target,
            sysroot_path=None,
            profile="dev",
            project_root=tmp_path,
            diagnostics_enabled=False,
            phase_starts={},
            link_timeout=None,
            warnings=[],
        )
        assert failure is None and result is not None
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
        target="native",
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
        diagnostics_payload=None,
        diagnostics_path=None,
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
    assert binary.read_bytes() == b"raw binary signed"
    assert prepare().link_skipped and len(linked) == 1
    before = app.stat()
    os.utime(app, ns=(before.st_atime_ns, before.st_mtime_ns + 2_000_000_000))
    assert prepare().link_skipped and len(linked) == 1
    before = binary.stat()
    binary.write_bytes(b"bad binary signed")
    os.utime(binary, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert not prepare().link_skipped and len(linked) == 2
