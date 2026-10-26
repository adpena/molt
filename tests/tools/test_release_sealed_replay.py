"""Independent byte/protocol rejection oracles; no Docker/compiled-guest claims."""

from __future__ import annotations

import copy
import hashlib
import io
import os
from pathlib import Path
import tarfile
import json
import subprocess
from types import SimpleNamespace

import pytest

from tools import cross_run
from tools.proof_queue_pkg import custody_cas, supervisor_custody
from tools.release import consumer_replay, execution_root, provision_execution_archives
from molt import tool_releases
from tests.native_artifact_fixtures import elf_dynamic_image, elf_header


def _wasm_stage_inputs(tmp_path):
    source = tmp_path / "emitted"
    source.mkdir()
    data = b"\0asm\x01\0\0\0"
    module = source / "program.wasm"
    module.write_bytes(data)
    artifact = {
        "path": str(module),
        "sha256": hashlib.sha256(data).hexdigest(),
        "size": len(data),
    }
    raw = _json(
        {"mode": "linked", "modules": {"linked": {**artifact, "path": module.name}}}
    )
    manifest = source / "manifest.json"
    manifest.write_bytes(raw)
    return (
        module,
        manifest,
        artifact,
        {"sha256": hashlib.sha256(raw).hexdigest(), "size": len(raw)},
        data,
        raw,
    )


@pytest.mark.parametrize("selected", ["manifest", "module"])
@pytest.mark.parametrize("mutation", ["replace", "overwrite", "grow"])
def test_wasm_stage_rejects_changed_source_before_it_can_be_sealed(
    tmp_path, monkeypatch, selected, mutation
):
    module, manifest, artifact, descriptor, _, _ = _wasm_stage_inputs(tmp_path)
    target = manifest if selected == "manifest" else module
    real_stage = execution_root.stage_file
    reached = []

    def mutate_then_stage(root, name, source, **kwargs):
        if source == target:
            reached.append(name)
            raw = source.read_bytes()
            changed = raw + b"growth" if mutation == "grow" else b"x" * len(raw)
            if mutation == "replace":
                replacement = tmp_path / "replacement"
                replacement.write_bytes(changed)
                os.replace(replacement, source)
            else:
                times = source.stat()
                source.write_bytes(changed)
                os.utime(source, ns=(times.st_atime_ns, times.st_mtime_ns))
        return real_stage(root, name, source, **kwargs)

    monkeypatch.setattr(execution_root, "stage_file", mutate_then_stage)
    with pytest.raises(ValueError, match="artifact changed|byte limit"):
        consumer_replay._stage_wasm(
            tmp_path / "root", "app/cell/", artifact, descriptor
        )
    assert len(reached) == 1
    assert not (tmp_path / "root.tar").exists()


def test_wasm_stage_uses_captured_bytes_when_sources_change_after_snapshot(
    tmp_path, monkeypatch
):
    module, manifest, artifact, descriptor, data, raw = _wasm_stage_inputs(tmp_path)
    real_stage = execution_root.stage_file

    def stage_then_change_source(root, name, source, **kwargs):
        result = real_stage(root, name, source, **kwargs)
        source.write_bytes(b"changed source after retained copy")
        return result

    monkeypatch.setattr(execution_root, "stage_file", stage_then_change_source)
    root = tmp_path / "root"
    rows = consumer_replay._stage_wasm(root, "app/cell/", artifact, descriptor)
    assert (root / "app/cell/manifest.json").read_bytes() == raw
    assert (root / "app/cell/program.wasm").read_bytes() == data
    seal = execution_root.seal_root(root, tmp_path / "root.tar", expected_files=rows)
    execution_root.validate_sealed_tar(
        tmp_path / "root.tar", root, expected_archive=seal["archive"]
    )


def test_staged_manifest_decode_is_bound_to_expected_bytes(tmp_path, monkeypatch):
    _, _, artifact, descriptor, _, _ = _wasm_stage_inputs(tmp_path)
    real_capture = consumer_replay.capture_exact

    def replace_before_decode(path, **kwargs):
        path.chmod(0o644)
        path.write_bytes(
            _json({"mode": "linked", "modules": {"linked": {"path": "other.wasm"}}})
        )
        return real_capture(path, **kwargs)

    monkeypatch.setattr(consumer_replay, "capture_exact", replace_before_decode)
    with pytest.raises(ValueError, match="artifact changed"):
        consumer_replay._stage_wasm(
            tmp_path / "root", "app/cell/", artifact, descriptor
        )


def test_seal_cannot_readmit_changed_previously_staged_payload(tmp_path):
    root = tmp_path / "root"
    admitted = execution_root.write_payload(
        root, "app/program", b"admitted", executable=True
    )
    path = root / "app/program"
    path.chmod(0o755)
    path.write_bytes(b"forged!!")
    with pytest.raises(ValueError, match="admitted payloads"):
        execution_root.seal_root(root, tmp_path / "root.tar", expected_files=[admitted])
    assert not (tmp_path / "root.tar").exists()


def test_root_inventory_and_tar_share_posix_wire_order(tmp_path):
    root = tmp_path / "root"
    rows = [
        execution_root.write_payload(root, name, name.encode(), executable=False)
        for name in ("source/a/b", "source/a.py", "source/a-b")
    ]
    expected = ["source/a-b", "source/a.py", "source/a/b"]
    assert [row["path"] for row in execution_root.root_inventory(root)] == expected
    seal = execution_root.seal_root(root, tmp_path / "root.tar", expected_files=rows)
    execution_root.validate_sealed_tar(
        tmp_path / "root.tar", root, expected_archive=seal["archive"]
    )


def test_evidence_inventory_does_not_read_discarded_materializations(
    tmp_path, monkeypatch
):
    (tmp_path / "rootfs").mkdir()
    (tmp_path / "rootfs/large-input").write_bytes(b"materialized input")
    (tmp_path / "rootfs.tar").write_bytes(b"materialized tar")
    (tmp_path / "receipt.json").write_bytes(b"original receipt")
    original = execution_root.file_identity
    observed = []

    def read(path):
        assert path == tmp_path / "receipt.json"
        observed.append(path)
        return original(path)

    monkeypatch.setattr(execution_root, "file_identity", read)
    rows = consumer_replay._evidence_inventory(tmp_path)
    assert [row["path"] for row in rows] == ["receipt.json"]
    assert observed == [tmp_path / "receipt.json"]


def test_closed_roots_share_payloads_and_reconstruct_original_tar_after_move(tmp_path):
    candidate = tmp_path / "candidate"
    resources = candidate / "execution-inputs"
    originals = []
    seals = []
    observations = []
    # Independent literal bytes are used at different paths and modes. One
    # stored payload must serve all eight compilation coordinates.
    payload = b"original admitted source\x00\xff"
    for index in range(8):
        evidence = candidate / "compilation-evidence" / str(index)
        row = execution_root.write_payload(
            evidence / "rootfs",
            f"source-{index}/input",
            payload,
            executable=bool(index % 2),
        )
        seal = execution_root.seal_root(
            evidence / "rootfs",
            evidence / "rootfs.tar",
            expected_files=[row],
        )
        originals.append((evidence / "rootfs.tar").read_bytes())
        seals.append(seal)
        observations.append(
            execution_root.retain_root(
                evidence / "rootfs",
                evidence / "rootfs.tar",
                sealed=seal,
                resources=resources,
            )
        )
        # A failed/interrupting record publication still has both originals.
        assert (evidence / "rootfs.tar").read_bytes() == originals[index]
        assert (evidence / "rootfs" / f"source-{index}/input").read_bytes() == payload
        execution_root.retire_root(
            evidence / "rootfs", evidence / "rootfs.tar", sealed=seal
        )
        assert not (evidence / "rootfs").exists()
        assert not (evidence / "rootfs.tar").exists()
    assert observations[0] == {
        "new_files": 1,
        "new_bytes": len(payload),
        "reused_files": 0,
        "reused_bytes": 0,
    }
    assert all(
        row
        == {
            "new_files": 0,
            "new_bytes": 0,
            "reused_files": 1,
            "reused_bytes": len(payload),
        }
        for row in observations[1:]
    )
    assert [path.read_bytes() for path in resources.rglob("payload")] == [payload]
    moved = tmp_path / "moved"
    candidate.rename(moved)
    for index, seal in enumerate(seals):
        evidence = moved / "compilation-evidence" / str(index)
        with execution_root.materialized_root(
            evidence / "rootfs",
            evidence / "rootfs.tar",
            sealed=seal,
            resources=moved / "execution-inputs",
        ):
            assert (evidence / "rootfs.tar").read_bytes() == originals[index]
            assert (
                evidence / "rootfs" / f"source-{index}/input"
            ).read_bytes() == payload
        assert not (evidence / "rootfs").exists()


@pytest.mark.parametrize(
    "defect",
    [
        "missing",
        "bytes",
        "mode",
        "tar",
        "omitted",
        "schema",
        "destination-mode",
        "size",
    ],
)
def test_retained_root_resources_refuse_substitution(tmp_path, defect):
    root, archive, resources = (
        tmp_path / "root",
        tmp_path / "root.tar",
        tmp_path / "cas",
    )
    row = execution_root.write_payload(
        root, "source/input", b"admitted", executable=False
    )
    seal = execution_root.seal_root(root, archive, expected_files=[row])
    execution_root.retain_root(root, archive, sealed=seal, resources=resources)
    execution_root.retire_root(root, archive, sealed=seal)
    payload = next(resources.rglob("payload"))
    if defect == "missing":
        payload.rename(payload.with_name("lost"))
    elif defect == "bytes":
        payload.chmod(0o644)
        payload.write_bytes(b"replaced")
        payload.chmod(0o444)
    elif defect == "mode":
        payload.chmod(0o644)
    elif defect == "tar":
        seal["archive"]["sha256"] = "0" * 64
    elif defect == "omitted":
        seal["files"] = []
        seal["files_sha256"] = consumer_replay.canonical_json_sha256([])
    elif defect in {"destination-mode", "size"}:
        seal["files"][0]["mode" if defect == "destination-mode" else "size"] = (
            0o555 if defect == "destination-mode" else 1
        )
        seal["files_sha256"] = consumer_replay.canonical_json_sha256(seal["files"])
    else:
        seal["schema"] = "molt.release-execution-root.v1"
    with pytest.raises((ValueError, FileNotFoundError)):
        with execution_root.materialized_root(
            root, archive, sealed=seal, resources=resources
        ):
            pytest.fail("substituted retained root was admitted")


def test_failed_replay_preserves_materialized_root_and_original_tar(tmp_path):
    root, archive, resources = (
        tmp_path / "root",
        tmp_path / "root.tar",
        tmp_path / "cas",
    )
    row = execution_root.write_payload(
        root, "source/input", b"admitted", executable=False
    )
    seal = execution_root.seal_root(root, archive, expected_files=[row])
    execution_root.retain_root(root, archive, sealed=seal, resources=resources)
    execution_root.retire_root(root, archive, sealed=seal)
    with pytest.raises(RuntimeError, match="original receipt failed"):
        with execution_root.materialized_root(
            root, archive, sealed=seal, resources=resources
        ):
            raise RuntimeError("original receipt failed")
    assert (root / "source/input").read_bytes() == b"admitted"
    execution_root.validate_sealed_tar(archive, root, expected_archive=seal["archive"])


def test_consumer_resource_archive_exceeds_generic_envelope_without_nesting(tmp_path):
    """Actual flat resources cross 4096 nodes; no native/product claim is made."""
    from tools.release import archive as archive_owner
    import zipfile

    candidate_dir = tmp_path / "candidate"
    root, tar = tmp_path / "root", tmp_path / "rootfs.tar"
    rows = [
        execution_root.write_payload(
            root, f"source/{i}", i.to_bytes(2, "big"), executable=False
        )
        for i in range(2050)
    ]
    seal = execution_root.seal_root(root, tar, expected_files=rows)
    execution_root.retain_root(
        root, tar, sealed=seal, resources=candidate_dir / "execution-inputs"
    )
    candidate = {
        "version": "0.0.1",
        "target": {"platform": "linux", "arch": "x86_64"},
        "source_date_epoch": 1_700_000_000,
    }
    consumer = {
        "standalone": {"path": "consumer-evidence/replay.json"},
        "python_proofs": [],
        "pip_proof": {
            "compilation": {"path": "compilation-evidence/pip-3.12/compilation.json"}
        },
    }
    for name in (
        "consumer-evidence/replay.json",
        "compilation-evidence/pip-3.12/compilation.json",
        "compilation-evidence/pip-3.12-cold/compilation.json",
    ):
        path = candidate_dir / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"root": seal}), encoding="utf-8")
    (candidate_dir / "candidate.json").write_text(
        json.dumps(candidate), encoding="utf-8"
    )
    receipt = candidate_dir / "consumer-verification.json"
    receipt.write_text(json.dumps(consumer), encoding="utf-8")
    retained = consumer_replay.publish_archive(
        candidate_dir=candidate_dir, receipt=receipt, candidate=candidate
    )
    with zipfile.ZipFile(retained) as handle:
        names = handle.namelist()
        assert len([name for name in names if name.endswith("/payload")]) == 2050
        assert not any(name.endswith((".tar", ".zip")) for name in names)
        assert (
            sum(
                row.file_size
                for row in handle.infolist()
                if row.filename.endswith("/payload")
            )
            == 4100
        )
    with pytest.raises(ValueError, match="member-count"):
        archive_owner.extract_zip_strict(retained, tmp_path / "generic")
    moved = tmp_path / "moved"
    archive_owner.extract_zip_strict(
        retained, moved, policy=consumer_replay.CONSUMER_ARCHIVE_POLICY
    )
    consumer_replay.validate_resources(evidence_root=moved, consumer=consumer)


@pytest.mark.parametrize("boundary", ["member", "archive"])
def test_sealed_tar_hash_and_consumption_retain_the_same_generation(
    tmp_path, monkeypatch, boundary
):
    root = tmp_path / "root"
    row = execution_root.write_payload(
        root, "app/program", b"admitted", executable=True
    )
    archive = tmp_path / "root.tar"
    if boundary == "archive":
        seal = execution_root.seal_root(root, archive, expected_files=[row])
    target = root / "app/program" if boundary == "member" else archive
    real_identity = execution_root.stable_regular_file_handle_identity
    reached = []

    def capture_then_replace(opened, **kwargs):
        identity = real_identity(opened, **kwargs)
        if opened.path == target:
            reached.append(target)
            replacement = tmp_path / "replacement"
            replacement.write_bytes(target.read_bytes())
            os.replace(replacement, target)
        return identity

    monkeypatch.setattr(
        execution_root, "stable_regular_file_handle_identity", capture_then_replace
    )
    with pytest.raises((ValueError, OSError)):
        if boundary == "member":
            execution_root.seal_root(root, archive, expected_files=[row])
        else:
            execution_root.validate_sealed_tar(
                archive, root, expected_archive=seal["archive"]
            )
    assert reached == [target]


def test_execute_refuses_staged_drift_before_transport_construction(
    tmp_path, monkeypatch
):
    root = tmp_path / "rootfs"
    row = execution_root.write_payload(
        root, "app/program", b"admitted", executable=True
    )
    seal = execution_root.seal_root(root, tmp_path / "rootfs.tar", expected_files=[row])
    (root / "app/program").chmod(0o755)
    (root / "app/program").write_bytes(b"forged!!")
    monkeypatch.setattr(
        consumer_replay,
        "SealedRootTransport",
        lambda *_: pytest.fail("changed inputs reached transport"),
    )
    with pytest.raises(ValueError, match="before execution"):
        consumer_replay.execute(
            evidence=tmp_path,
            replay={"root": seal},
            supervisor=tmp_path / "unused",
            expected_stdout="",
        )


def test_native_dependency_parser_receives_only_admitted_bounded_payload(
    tmp_path, monkeypatch
):
    root = tmp_path / "root"
    (root / "lib/x86_64-linux-gnu").mkdir(parents=True)
    row = execution_root.write_payload(
        root, "app/program", b"admitted", executable=True
    )
    path = root / "app/program"
    path.chmod(0o755)
    path.write_bytes(b"forged!!")
    monkeypatch.setattr(
        execution_root,
        "elf_interpreter",
        lambda *a, **k: pytest.fail("unadmitted input reached ELF parser"),
    )
    with pytest.raises(ValueError, match="ELF bytes differ"):
        execution_root.audit_native_closure(
            root, arch="x86_64", executable_paths=["app/program"], expected_files=[row]
        )


def test_export_capture_must_match_retained_inventory(tmp_path):
    raw, _, _ = _export()
    path = tmp_path / "export"
    path.write_bytes(raw)
    expected = {"size": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}
    assert (
        supervisor_custody.read_supervisor_export(path, expected=expected)[0]
        == b"guest stderr\n"
    )
    changed, _, _ = _export(prefix=b"other stderr\n")
    path.write_bytes(changed)
    with pytest.raises(ValueError, match="retained inventory"):
        supervisor_custody.read_supervisor_export(path, expected=expected)


def _json(value):
    return json.dumps(value, separators=(",", ":")).encode()


@pytest.mark.parametrize("changed", [False, True])
def test_replay_provisioning_consumes_the_real_generation_cas_format(
    tmp_path, monkeypatch, changed
):
    target = tmp_path / "target"
    cas = tmp_path / "custody-cas"
    generation = {
        "schema": custody_cas.ARTIFACT_SCHEMA,
        "fixture": "compressed generation, no compilation",
    }
    reference = custody_cas.put_json(cas, generation).as_dict()
    binary = tmp_path / "not-executed"
    telemetry = {"build_target_dir": str(target), "generation_artifact": reference}
    monkeypatch.setattr(
        consumer_replay.supervisor_generation,
        "provision",
        lambda **_: (binary, telemetry),
    )
    if changed:
        blob = Path(reference["path"])
        blob.chmod(0o644)
        blob.write_bytes(b"changed compressed generation")
        with pytest.raises(ValueError):
            consumer_replay.provision_verifier()
    else:
        assert consumer_replay.provision_verifier() == (binary, generation)


def _export(events=b'{"source":"framing-only-fixture"}\n', prefix=b"guest stderr\n"):
    # Literal independently encoded wire footer, not the producer formatter.
    digest = hashlib.sha256(events).hexdigest()
    receipt = _json(
        {
            "event_log": {
                "file": f"receipt.json.events.{digest}.jsonl",
                "sha256": digest,
                "bytes": len(events),
            }
        }
    )
    footer = (
        b"\nMOLT-PROOF-SUPERVISOR-EXPORT-V1\n"
        + f"{len(receipt):016x}{len(events):016x}\n".encode()
    )
    return prefix + receipt + events + footer, receipt, events


def test_export_exact_eof_and_raw_bytes_are_framing_only(tmp_path):
    data, receipt, events = _export(
        prefix=b"lookalike\nMOLT-PROOF-SUPERVISOR-EXPORT-V1\n\xff"
    )
    path = tmp_path / "stderr"
    path.write_bytes(data)
    prefix, observed_receipt, observed_events = (
        supervisor_custody.read_supervisor_export(path)
    )
    assert prefix.endswith(b"\xff")
    assert observed_receipt == receipt and observed_events == events
    # A framed object is deliberately not semantic success.
    with pytest.raises(ValueError, match="did not close"):
        consumer_replay.require_success(json.loads(receipt))


@pytest.mark.parametrize(
    "mutation",
    [
        lambda x: x + b"junk",
        lambda x: x[:-1],
        lambda x: x[:-66] + b"x" * 66,
        lambda x: x.replace(b"framing-only", b"altered-only"),
    ],
)
def test_export_rejects_ambiguous_footer_or_changed_event_bytes(tmp_path, mutation):
    data, _, _ = _export()
    path = tmp_path / "stderr"
    path.write_bytes(mutation(data))
    with pytest.raises(ValueError):
        supervisor_custody.read_supervisor_export(path)


def test_sealed_tar_retains_one_exact_root_and_rejects_appended_or_substituted_bytes(
    tmp_path,
):
    root = tmp_path / "root"
    root.mkdir()
    admitted = execution_root.write_payload(
        root, "app/program", b"guest artifact", executable=True
    )
    archive = tmp_path / "root.tar"
    seal = execution_root.seal_root(root, archive, expected_files=[admitted])
    execution_root.validate_sealed_tar(archive, root, expected_archive=seal["archive"])
    assert seal["archive"]["sha256"] == hashlib.sha256(archive.read_bytes()).hexdigest()
    # Receiver transport changes host writability, not executable identity.
    (root / "app/program").chmod(0o755)
    execution_root.validate_sealed_tar(archive, root, expected_archive=seal["archive"])
    original = archive.read_bytes()
    archive.write_bytes(original + b"concealed trailing content")
    with pytest.raises(ValueError, match="trailing"):
        execution_root.validate_sealed_tar(
            archive, root, expected_archive=execution_root.file_identity(archive)
        )
    archive.write_bytes(original)
    (root / "app/program").write_bytes(b"different artifact")
    with pytest.raises(ValueError, match="differs"):
        execution_root.validate_sealed_tar(
            archive, root, expected_archive=seal["archive"]
        )


@pytest.mark.parametrize(
    "name",
    [
        "bin/python3",
        "lib/libpython3.13.so",
        "lib/renamed-runtime.so",
        "app/hidden-package/__init__.py",
    ],
)
def test_sealed_tar_detects_positive_contamination_independent_of_filename_rules(
    tmp_path, name
):
    root = tmp_path / "root"
    root.mkdir()
    admitted = execution_root.write_payload(
        root, "app/program", b"ordinary product", executable=True
    )
    archive = tmp_path / "root.tar"
    seal = execution_root.seal_root(root, archive, expected_files=[admitted])
    execution_root.validate_sealed_tar(archive, root, expected_archive=seal["archive"])
    # Positive contamination: retained root now has additional bytes. Admission
    # must reject even the innocuous renamed library, not merely Python strings.
    execution_root.write_payload(root, name, b"contaminating bytes", executable=True)
    with pytest.raises(ValueError):
        execution_root.validate_sealed_tar(
            archive, root, expected_archive=seal["archive"]
        )


def test_root_inventory_rejects_file_and_root_indirection(tmp_path):
    root = tmp_path / "root"
    root.mkdir()
    outside = tmp_path / "outside"
    outside.write_bytes(b"host Python could be anywhere")
    link = root / "renamed"
    link.symlink_to(outside)
    with pytest.raises(ValueError):
        execution_root.root_inventory(root)
    link.unlink()
    alias = tmp_path / "alias"
    alias.symlink_to(root, target_is_directory=True)
    with pytest.raises(ValueError):
        execution_root.root_inventory(alias)


def test_missing_and_changed_os_archive_fail_without_network(tmp_path, monkeypatch):
    import urllib.request

    monkeypatch.setattr(
        urllib.request,
        "build_opener",
        lambda *a, **k: pytest.fail("archive validation must never fetch"),
    )
    raw = b"pinned package payload"
    record = {
        "filename": "libc.deb",
        "url": "https://example.invalid/libc.deb",
        "size": len(raw),
        "sha256": hashlib.sha256(raw).hexdigest(),
    }
    with pytest.raises(tool_releases.ToolReleaseError, match="no automatic fetch"):
        with execution_root.open_pinned_archive(
            tmp_path / record["filename"], size=record["size"], sha256=record["sha256"]
        ):
            pass
    (tmp_path / "libc.deb").write_bytes(raw)
    with execution_root.open_pinned_archive(
        tmp_path / record["filename"], size=record["size"], sha256=record["sha256"]
    ) as opened:
        assert opened.path == tmp_path / "libc.deb"
        assert opened.stream.read() == raw
    (tmp_path / "libc.deb").write_bytes(b"replacement")
    with pytest.raises(tool_releases.ToolReleaseError, match="identity mismatch"):
        with execution_root.open_pinned_archive(
            tmp_path / record["filename"], size=record["size"], sha256=record["sha256"]
        ):
            pass


def _systemd_observation():
    # Independent protocol fixture. No process or kernel absence is proved here.
    nonce = "a" * 32
    root = "/run/molt-release-" + "b" * 32 + "/rootfs"
    command = [
        "/bin/molt-proof-supervisor",
        "run-export",
        "--policy",
        "/policies/test.json",
        "--receipt",
        "/evidence/receipt.json",
    ]
    unit = {
        "Id": "molt-release-" + nonce + ".service",
        "Description": "molt-sealed:" + nonce,
        "InvocationID": [17] * 16,
        "Transient": True,
        "DropInPaths": [],
        "JoinsNamespaceOf": [],
        "TriggeredBy": [],
        "ActiveState": "active",
        "SubState": "exited",
    }
    service = {
        "Type": "exec",
        "ExitType": "cgroup",
        "RemainAfterExit": True,
        "Restart": "no",
        "RootDirectory": root,
        "RootImage": "",
        "RootEphemeral": True,
        "WorkingDirectory": "/app",
        "User": "65534",
        "Group": "65534",
        "SupplementaryGroups": [],
        "PAMName": "",
        "SetLoginEnvironment": False,
        "Environment": ["PATH=/absent", "HOME=/absent", "LANG=C", "LC_ALL=C"],
        "EnvironmentFiles": [],
        "PassEnvironment": [],
        "ExecSearchPath": [],
        "PrivatePIDs": "yes",
        "PrivateUsers": True,
        "PrivateUsersEx": "self",
        "PrivateNetwork": True,
        "PrivateIPC": True,
        "PrivateMounts": True,
        "PrivateDevices": True,
        "ProtectHostname": True,
        "ProtectHostnameEx": ["yes", ""],
        "ProtectControlGroups": True,
        "ProtectControlGroupsEx": "strict",
        "ProtectKernelTunables": True,
        "ProtectKernelLogs": True,
        "ProtectClock": True,
        "MountAPIVFS": True,
        "ProtectSystem": "strict",
        "ReadOnlyPaths": ["/"],
        "ReadWritePaths": [],
        "NoExecPaths": ["/tmp", "/evidence"],
        "InaccessiblePaths": ["-/run/host", "-/proc/kcore", "-/sys/firmware"],
        "TemporaryFileSystem": [
            ["/tmp", "rw,noexec,nosuid,nodev,size=64m,mode=1777"],
            ["/evidence", "rw,noexec,nosuid,nodev,size=16m,mode=1777"],
            ["/dev/shm", "rw,noexec,nosuid,nodev,size=16m,mode=1777"],
        ],
        "BindPaths": [],
        "BindReadOnlyPaths": [],
        "MountImages": [],
        "ExtensionImages": [],
        "ExtensionDirectories": [],
        "NetworkNamespacePath": "",
        "IPCNamespacePath": "",
        "UserNamespacePath": "",
        "CapabilityBoundingSet": 0,
        "AmbientCapabilities": 0,
        "NoNewPrivileges": True,
        "RestrictSUIDSGID": True,
        "RestrictNamespaces": 0,
        "DevicePolicy": "closed",
        "DeviceAllow": [],
        "MemoryMax": 1073741824,
        "MemorySwapMax": 0,
        "TasksMax": 64,
        "CPUQuotaPerSecUSec": 1000000,
        "KillMode": "control-group",
        "SendSIGKILL": True,
        "TimeoutStartUSec": 60000000,
        "TimeoutStopUSec": 10000000,
        "RuntimeMaxUSec": 60000000,
        "Delegate": False,
        "StandardInput": "null",
        "FileDescriptorStoreMax": 0,
        "LimitFSIZE": 16777216,
        "LimitFSIZESoft": 16777216,
        "ExecStartPre": [],
        "ExecStartPost": [],
        "ExecStop": [],
        "ExecStopPost": [],
        "ExecCondition": [],
        "ExecReload": [],
        "ExecStart": [[command[0], command, False, 1, 1, 2, 2, 42, 1, 0]],
        "OpenFile": [],
        "ExtraFileDescriptorNames": [],
        "NFileDescriptorStore": 0,
        "BindLogSockets": False,
        "RootDirectoryStartOnly": False,
        "NotifyAccess": "none",
        "MainPID": 0,
        "ControlPID": 0,
        "ExecMainCode": 1,
        "ExecMainStatus": 0,
        "Result": "success",
        "NRestarts": 0,
    }
    return nonce, root, command, {"Unit": unit, "Service": service}


def _typed_observation(interfaces):
    def value(v):
        if type(v) is bool:
            signature = "b"
        elif type(v) is int:
            signature = "t"
        elif isinstance(v, str):
            signature = "s"
        else:
            signature = "av"
        return {"type": signature, "data": v}

    return json.dumps(
        {
            name: json.dumps(
                {"type": "a{sv}", "data": [{k: value(v) for k, v in fields.items()}]}
            )
            for name, fields in interfaces.items()
        }
    )


@pytest.mark.parametrize(
    "key,value",
    [
        ("PrivateNetwork", False),
        ("PrivatePIDs", "no"),
        ("PrivateUsers", False),
        ("RootEphemeral", False),
        ("ProtectSystem", "no"),
        ("ReadOnlyPaths", []),
        ("User", "0"),
        ("CapabilityBoundingSet", 1),
        ("NoNewPrivileges", False),
        ("BindPaths", [["/usr", "/host", False, 0]]),
        ("BindReadOnlyPaths", [["/usr/bin/python3", "/bin/python3", False, 0]]),
        ("DeviceAllow", [["/dev/sda", "rw"]]),
        ("ProtectKernelTunables", False),
        ("Environment", ["PATH=/usr/bin", "PYTHONPATH=/host"]),
        (
            "ExecStart",
            [["/bin/python3", ["/bin/python3"], False, 1, 1, 2, 2, 42, 1, 0]],
        ),
        ("MemoryMax", 1073741825),
        ("MemorySwapMax", 1),
        ("TasksMax", 65),
        ("CPUQuotaPerSecUSec", 1000001),
        ("NoExecPaths", []),
        ("TemporaryFileSystem", [["/tmp", "rw,nosuid,nodev,size=64m,mode=1777"]]),
        ("ExitType", "main"),
        ("ExtraFileDescriptorNames", ["host-root"]),
    ],
)
def test_effective_systemd_isolation_rejects_host_and_resource_contamination(
    key, value
):
    nonce, root, command, interfaces = _systemd_observation()
    cross_run.validate_sealed_systemd_configuration(
        _typed_observation(interfaces), nonce=nonce, root=root, command=command
    )
    interfaces["Service"][key] = value
    with pytest.raises(ValueError):
        cross_run.validate_sealed_systemd_configuration(
            _typed_observation(interfaces), nonce=nonce, root=root, command=command
        )


def test_systemd_required_properties_cannot_be_omitted():
    nonce, root, command, interfaces = _systemd_observation()
    for name in (
        "EnvironmentFiles",
        "BindPaths",
        "TemporaryFileSystem",
        "RootDirectory",
        "PrivatePIDs",
        "NoNewPrivileges",
        "ExecStart",
        "ExitType",
    ):
        modified = copy.deepcopy(interfaces)
        del modified["Service"][name]
        with pytest.raises(ValueError):
            cross_run.validate_sealed_systemd_configuration(
                _typed_observation(modified), nonce=nonce, root=root, command=command
            )


@pytest.mark.parametrize(
    "key,value",
    [
        ("MainPID", 77),
        ("ControlPID", 77),
        ("ExecMainStatus", 1),
        ("ExecMainCode", 2),
        ("NRestarts", 1),
        ("Result", "oom-kill"),
        ("MainPID", False),
        ("ControlPID", False),
        ("ExecMainStatus", False),
        ("ExecMainCode", True),
        ("NRestarts", False),
    ],
)
def test_systemd_terminal_rejects_failure_or_unclosed_custody(key, value):
    nonce, root, command, interfaces = _systemd_observation()
    cross_run.validate_sealed_systemd_terminal(
        _typed_observation(interfaces), invocation=[17] * 16
    )
    interfaces["Service"][key] = value
    with pytest.raises(ValueError):
        cross_run.validate_sealed_systemd_terminal(
            _typed_observation(interfaces), invocation=[17] * 16
        )


def test_systemd_custody_and_provider_omissions_reject():
    nonce, root, command, interfaces = _systemd_observation()
    for key, value in [
        ("DropInPaths", ["/etc/systemd/system/service.d/override.conf"]),
        ("InvocationID", [18] * 15),
        ("InvocationID", [0] * 16),
        ("Id", "foreign.service"),
    ]:
        modified = copy.deepcopy(interfaces)
        modified["Unit"][key] = value
        with pytest.raises(ValueError):
            cross_run.validate_sealed_systemd_configuration(
                _typed_observation(modified), nonce=nonce, root=root, command=command
            )
    for provider in (
        {},
        {"version": "259.5-0ubuntu3.4"},
        {
            "schema": "molt.sealed-systemd.v1",
            "version": "255",
            "platform": "linux",
            "arch": "aarch64",
            "kernel": "6.8",
            "tools": {},
        },
    ):
        with pytest.raises(ValueError):
            cross_run.validate_sealed_systemd_provider(provider, arch="aarch64")


def test_systemd_patch_version_is_provenance_not_a_capability_allowlist():
    provider = {
        "schema": "molt.sealed-systemd.v1",
        "version": "259.5-0ubuntu3.4",
        "platform": "linux",
        "arch": "aarch64",
        "kernel": "7.0.0-38-generic",
        "tools": {
            path: "a" * 64
            for path in (
                "/usr/bin/systemd-run",
                "/usr/bin/systemctl",
                "/usr/bin/busctl",
                "/usr/bin/tar",
                "/usr/bin/true",
            )
        },
    }
    cross_run.validate_sealed_systemd_provider(provider, arch="aarch64")
    cross_run.validate_sealed_systemd_provider(
        {**provider, "version": "259.9-0ubuntu0.1"}, arch="aarch64"
    )
    for version in (None, "", False, "x" * 129):
        with pytest.raises(ValueError):
            cross_run.validate_sealed_systemd_provider(
                {**provider, "version": version}, arch="aarch64"
            )
    # A syntactically recorded version grants no isolation capability.
    nonce, root, command, interfaces = _systemd_observation()
    del interfaces["Service"]["PrivatePIDs"]
    with pytest.raises(ValueError):
        cross_run.validate_sealed_systemd_configuration(
            _typed_observation(interfaces), nonce=nonce, root=root, command=command
        )


def _systemd_import_observation():
    # Literal independent importer expectation, including archive and command.
    nonce = "b" * 32
    digest = "d" * 64
    root = f"/run/molt-release-{nonce}/rootfs"
    command = [
        "/usr/bin/tar",
        "--extract",
        "--file=-",
        "--directory=" + root,
        "--no-same-owner",
        "--same-permissions",
        "--delay-directory-restore",
    ]
    unit = {
        "Id": f"molt-release-import-{nonce}.service",
        "Description": f"molt-sealed-import:{nonce}:{digest}",
        "InvocationID": [22] * 16,
        "Transient": True,
        "DropInPaths": [],
        "JoinsNamespaceOf": [],
        "TriggeredBy": [],
        "ActiveState": "active",
        "SubState": "exited",
    }
    service = {
        "Type": "oneshot",
        "RemainAfterExit": True,
        "RuntimeDirectory": [f"molt-release-{nonce}/rootfs"],
        "RuntimeDirectoryMode": 493,
        "RuntimeDirectoryPreserve": "no",
        "User": "root",
        "Group": "root",
        "NoNewPrivileges": True,
        "CapabilityBoundingSet": 0,
        "AmbientCapabilities": 0,
        "PrivateNetwork": True,
        "PrivateIPC": True,
        "PrivateDevices": True,
        "PrivateMounts": True,
        "ProtectSystem": "strict",
        "ProtectHome": "yes",
        "ProtectKernelTunables": True,
        "ProtectKernelLogs": True,
        "ProtectControlGroups": True,
        "ProtectControlGroupsEx": "strict",
        "RestrictSUIDSGID": True,
        "RestrictNamespaces": 0,
        "ReadWritePaths": [root],
        "StandardInput": "file",
        "StandardOutput": "null",
        "StandardError": "null",
        "Environment": ["LANG=C", "LC_ALL=C"],
        "EnvironmentFiles": [],
        "PassEnvironment": [],
        "UMask": 18,
        "TimeoutStartUSec": 120000000,
        "TimeoutStopUSec": 10000000,
        "Restart": "no",
        "MemoryMax": 2147483648,
        "MemorySwapMax": 0,
        "TasksMax": 16,
        "CPUQuotaPerSecUSec": 1000000,
        "LimitFSIZE": 1073741824,
        "LimitFSIZESoft": 1073741824,
        "KillMode": "control-group",
        "SendSIGKILL": True,
        "Delegate": False,
        "ExecStartPre": [],
        "ExecStartPost": [],
        "ExecStopPost": [],
        "ExecCondition": [],
        "ExecStart": [[command[0], command, False, 1, 1, 2, 2, 42, 1, 0]],
        "ExecStop": [["/usr/bin/true", ["/usr/bin/true"], False, 0, 0, 0, 0, 0, 0, 0]],
        "MainPID": 0,
        "ControlPID": 0,
        "ExecMainCode": 1,
        "ExecMainStatus": 0,
        "Result": "success",
        "NRestarts": 0,
    }
    return nonce, digest, root, {"Unit": unit, "Service": service}


def test_systemd_import_archive_substitution_and_omission_reject():
    nonce, digest, root, interfaces = _systemd_import_observation()
    raw = _typed_observation(interfaces)
    cross_run.validate_sealed_systemd_import(raw, nonce=nonce, sha256=digest, root=root)
    with pytest.raises(ValueError):
        cross_run.validate_sealed_systemd_import(
            raw, nonce=nonce, sha256="e" * 64, root=root
        )
    for name in (
        "EnvironmentFiles",
        "StandardInput",
        "ReadWritePaths",
        "CapabilityBoundingSet",
        "MemoryMax",
        "CPUQuotaPerSecUSec",
        "ExecStart",
        "ExecStop",
        "RuntimeDirectory",
    ):
        modified = copy.deepcopy(interfaces)
        del modified["Service"][name]
        with pytest.raises(ValueError):
            cross_run.validate_sealed_systemd_import(
                _typed_observation(modified), nonce=nonce, sha256=digest, root=root
            )
    for name, value in (
        ("ReadWritePaths", ["/"]),
        ("MemorySwapMax", 1),
        ("StandardInput", "null"),
        ("ExecMainStatus", 1),
        ("RuntimeDirectory", ["sibling/rootfs"]),
        ("ProtectSystem", "no"),
    ):
        modified = copy.deepcopy(interfaces)
        modified["Service"][name] = value
        with pytest.raises(ValueError):
            cross_run.validate_sealed_systemd_import(
                _typed_observation(modified), nonce=nonce, sha256=digest, root=root
            )


def test_systemd_teardown_requires_exact_completed_stop_job():
    nonce, root, command, interfaces = _systemd_observation()
    unit = interfaces["Unit"]["Id"]
    invocation = interfaces["Unit"]["InvocationID"]
    record = {
        "unit": unit,
        "invocation": invocation,
        "before": _typed_observation(interfaces),
        "stop_returncode": 0,
        "stop_stdout": "",
        "stop_stderr": "",
        "load_state": "not-found\n",
        "after": None,
    }
    cross_run.validate_sealed_systemd_stop(record, unit=unit, invocation=invocation)
    for name, value in (
        ("unit", "sibling.service"),
        ("invocation", [99] * 16),
        ("stop_returncode", 1),
        ("stop_returncode", False),
        ("stop_stderr", "denied"),
        ("load_state", "loaded\n"),
        ("after", record["before"]),
    ):
        modified = {**record, name: value}
        with pytest.raises(ValueError):
            cross_run.validate_sealed_systemd_stop(
                modified, unit=unit, invocation=invocation
            )
    for name in record:
        modified = dict(record)
        del modified[name]
        with pytest.raises(ValueError):
            cross_run.validate_sealed_systemd_stop(
                modified, unit=unit, invocation=invocation
            )
    after = copy.deepcopy(interfaces)
    after["Unit"].update(ActiveState="inactive", SubState="dead")
    retained = {**record, "load_state": "loaded\n", "after": _typed_observation(after)}
    cross_run.validate_sealed_systemd_stop(retained, unit=unit, invocation=invocation)
    after["Service"]["MainPID"] = False
    with pytest.raises(ValueError):
        cross_run.validate_sealed_systemd_stop(
            {**retained, "after": _typed_observation(after)},
            unit=unit,
            invocation=invocation,
        )
    after["Service"]["MainPID"] = 42
    with pytest.raises(ValueError):
        cross_run.validate_sealed_systemd_stop(
            {**retained, "after": _typed_observation(after)},
            unit=unit,
            invocation=invocation,
        )


@pytest.mark.parametrize("owner", ["guest", "import"])
@pytest.mark.parametrize(
    "key",
    [
        "MemoryMax",
        "MemorySwapMax",
        "TasksMax",
        "CPUQuotaPerSecUSec",
        "TimeoutStartUSec",
        "TimeoutStopUSec",
        "LimitFSIZE",
        "LimitFSIZESoft",
        "CapabilityBoundingSet",
        "AmbientCapabilities",
        "RestrictNamespaces",
        "MainPID",
        "ControlPID",
        "ExecMainCode",
        "ExecMainStatus",
        "NRestarts",
    ],
)
def test_systemd_numeric_fields_reject_booleans(owner, key):
    if owner == "guest":
        nonce, root, command, interfaces = _systemd_observation()

        def admit():
            raw = _typed_observation(interfaces)
            cross_run.validate_sealed_systemd_configuration(
                raw, nonce=nonce, root=root, command=command
            )
            cross_run.validate_sealed_systemd_terminal(raw, invocation=[17] * 16)
    else:
        nonce, digest, root, interfaces = _systemd_import_observation()

        def admit():
            cross_run.validate_sealed_systemd_import(
                _typed_observation(interfaces), nonce=nonce, sha256=digest, root=root
            )

    admit()
    interfaces["Service"][key] = bool(interfaces["Service"][key])
    with pytest.raises(ValueError):
        admit()


@pytest.mark.parametrize(
    "owner,key",
    [
        ("guest", "RuntimeMaxUSec"),
        ("guest", "FileDescriptorStoreMax"),
        ("guest", "NFileDescriptorStore"),
        ("import", "RuntimeDirectoryMode"),
        ("import", "UMask"),
    ],
)
def test_systemd_remaining_numeric_policy_rejects_booleans(owner, key):
    if owner == "guest":
        nonce, root, command, interfaces = _systemd_observation()
        interfaces["Service"][key] = bool(interfaces["Service"][key])
        with pytest.raises(ValueError):
            cross_run.validate_sealed_systemd_configuration(
                _typed_observation(interfaces), nonce=nonce, root=root, command=command
            )
    else:
        nonce, digest, root, interfaces = _systemd_import_observation()
        interfaces["Service"][key] = bool(interfaces["Service"][key])
        with pytest.raises(ValueError):
            cross_run.validate_sealed_systemd_import(
                _typed_observation(interfaces), nonce=nonce, sha256=digest, root=root
            )


@pytest.mark.parametrize(
    "owner,field",
    [("guest", "ExecStart"), ("import", "ExecStart"), ("import", "ExecStop")],
)
@pytest.mark.parametrize("index", range(2, 10))
def test_systemd_execution_rows_preserve_numeric_and_boolean_types(owner, field, index):
    # Positions 3..9 are four timestamps, PID, exit code and exit status.
    # Position 2 is genuinely boolean and rejects numeric zero.
    if owner == "guest":
        nonce, root, command, interfaces = _systemd_observation()

        def admit():
            cross_run.validate_sealed_systemd_configuration(
                _typed_observation(interfaces), nonce=nonce, root=root, command=command
            )
    else:
        nonce, digest, root, interfaces = _systemd_import_observation()

        def admit():
            cross_run.validate_sealed_systemd_import(
                _typed_observation(interfaces), nonce=nonce, sha256=digest, root=root
            )

    admit()
    row = interfaces["Service"][field][0]
    row[index] = 0 if index == 2 else bool(row[index])
    with pytest.raises(ValueError):
        admit()


@pytest.mark.parametrize(
    "owner", ["configuration", "import", "terminal", "terminal_argument"]
)
@pytest.mark.parametrize("index", [0, 1, 8, 15])
def test_systemd_invocation_octets_reject_equal_boolean_substitution(owner, index):
    # 0/1 ensure Python equality would accept the substituted false/true.
    invocation = [0, 1] * 8
    if owner == "import":
        nonce, digest, root, interfaces = _systemd_import_observation()
    else:
        nonce, root, command, interfaces = _systemd_observation()
    interfaces["Unit"]["InvocationID"] = invocation.copy()

    def admit():
        raw = _typed_observation(interfaces)
        if owner == "configuration":
            cross_run.validate_sealed_systemd_configuration(
                raw, nonce=nonce, root=root, command=command
            )
        elif owner == "import":
            cross_run.validate_sealed_systemd_import(
                raw, nonce=nonce, sha256=digest, root=root
            )
        else:
            cross_run.validate_sealed_systemd_terminal(raw, invocation=invocation)

    admit()
    target = (
        invocation
        if owner == "terminal_argument"
        else interfaces["Unit"]["InvocationID"]
    )
    target[index] = bool(target[index])
    with pytest.raises(ValueError):
        admit()


@pytest.mark.parametrize(
    "location",
    [
        "invocation",
        "argument",
        "before",
        "after",
        "MainPID",
        "ControlPID",
        "stop_returncode",
    ],
)
def test_systemd_stop_numeric_custody_rejects_booleans(location):
    _, _, _, before = _systemd_observation()
    invocation = [0, 1] * 8
    before["Unit"]["InvocationID"] = invocation.copy()
    after = copy.deepcopy(before)
    after["Unit"].update(ActiveState="inactive", SubState="dead")
    record = {
        "unit": before["Unit"]["Id"],
        "invocation": invocation.copy(),
        "before": _typed_observation(before),
        "stop_returncode": 0,
        "stop_stdout": "",
        "stop_stderr": "",
        "load_state": "loaded\n",
        "after": _typed_observation(after),
    }

    def admit():
        cross_run.validate_sealed_systemd_stop(
            record, unit=record["unit"], invocation=invocation
        )

    admit()
    if location in {"invocation", "argument"}:
        (invocation if location == "argument" else record["invocation"])[0] = False
    elif location in {"before", "after"}:
        source = before if location == "before" else after
        source["Unit"]["InvocationID"][1] = True
        record[location] = _typed_observation(source)
    elif location == "stop_returncode":
        record[location] = False
    else:
        after["Service"][location] = False
        record["after"] = _typed_observation(after)
    with pytest.raises(ValueError):
        admit()


@pytest.mark.parametrize("signature", ["u", "t", "i", "x", "q", "n", "y"])
@pytest.mark.parametrize("substitution", [False, True])
def test_systemd_typed_numeric_observation_rejects_boolean(signature, substitution):
    # Actual D-Bus numeric signatures cover counters, status and timestamps,
    # including metadata not used for release admission.
    envelope = {
        "Unit": json.dumps(
            {
                "type": "a{sv}",
                "data": [
                    {"StateChangeTimestamp": {"type": signature, "data": substitution}}
                ],
            }
        ),
        "Service": json.dumps({"type": "a{sv}", "data": [{}]}),
    }
    with pytest.raises(ValueError):
        cross_run.parse_systemd_observation(json.dumps(envelope))


@pytest.mark.parametrize("job_id", [False, True])
def test_systemd_job_observation_rejects_boolean_id(job_id):
    # Real systemd GetAll represents Job as (uo), independently of stop status.
    envelope = {
        "Unit": json.dumps(
            {
                "type": "a{sv}",
                "data": [{"Job": {"type": "(uo)", "data": [job_id, "/"]}}],
            }
        ),
        "Service": json.dumps({"type": "a{sv}", "data": [{}]}),
    }
    with pytest.raises(ValueError):
        cross_run.parse_systemd_observation(json.dumps(envelope))


def test_success_gate_rejects_failed_or_incomplete_terminal():
    # Exact native verifier invocation is exercised by the owning custody
    # tests. This independently rejects superficially successful failed receipts.
    valid = {
        "capability": {
            "admission": {
                "state": "admitted",
                "root_stable_process_id": "fixture:guest",
                "root_create_sequence": 1,
                "initial_image_sequence": 2,
            },
        },
        "complete": True,
        "state": "COMPLETE",
        "root_exit_code": 0,
        "error_count": 0,
        "violation_count": 0,
        "errors": [],
        "violations": [],
        "accounting": {"active_processes": 0},
    }
    consumer_replay.require_success(valid)
    for key, value in (
        ("complete", False),
        ("state", "FAILED"),
        ("capability", None),
        ("capability", {"admission": {"state": "eligible"}}),
        ("capability", {"admission": {"state": "ineligible", "reason": "denied"}}),
        ("root_exit_code", 1),
        ("root_exit_code", False),
        ("error_count", 1),
        ("error_count", False),
        ("violation_count", 1),
        ("violation_count", False),
        ("accounting", {"active_processes": False}),
        ("accounting", None),
    ):
        invalid = copy.deepcopy(valid)
        invalid[key] = value
        with pytest.raises(ValueError):
            consumer_replay.require_success(invalid)


def _tar_bytes(name, data):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:xz") as archive:
        member = tarfile.TarInfo(name)
        member.size = len(data)
        archive.addfile(member, io.BytesIO(data))
    return output.getvalue()


def _deb_bytes(name, data):
    # Independent literal ar framing, not the production archive serializer.
    result = bytearray(b"!<arch>\n")
    for member, payload in (
        ("debian-binary", b"2.0\n"),
        ("control.tar.xz", _tar_bytes("control", b"fixture")),
        ("data.tar.xz", _tar_bytes(name, data)),
    ):
        header = f"{member + '/':<16}{0:<12}{0:<6}{0:<6}{'100644':<8}{len(payload):<10}`\n".encode()
        assert len(header) == 60
        result.extend(header + payload)
        if len(payload) % 2:
            result.extend(b"\n")
    return bytes(result)


def _archive_fixture(tmp_path, *, arch="x86_64", missing=None):
    source = tmp_path / "source"
    (source / "config").mkdir(parents=True)
    archives = {}
    providers = []
    lines = ['schema = "molt.release-execution-roots.v1"']
    machine = 62 if arch == "x86_64" else 183
    packages = (
        ("libc6", "libc.so.6", None),
        ("libgcc-s1", "libgcc_s.so.1", b"libc.so.6"),
        ("libstdc++6", "libstdc++.so.6", b"libgcc_s.so.1"),
        ("libatomic1", "libatomic.so.1", b"libstdc++.so.6"),
    )
    for name, soname, dependency in packages:
        if soname == missing:
            continue
        filename = f"{name}_1.deb"
        archive_member, guest = f"usr/lib/{soname}", f"lib/{arch}-linux-gnu/{soname}"
        payload = bytes(
            elf_dynamic_image(dependency, machine=machine)
            if dependency is not None
            else elf_header(machine=machine, kind=3)
        )
        raw = _deb_bytes(archive_member, payload)
        url = "https://deb.debian.org/pool/" + filename
        archives[url] = raw
        providers.append((guest, payload))
        lines += [
            f"[[linux.{arch}.packages]]",
            f"name = {json.dumps(name)}",
            f"filename = {json.dumps(filename)}",
            f"url = {json.dumps(url)}",
            'provenance = "https://packages.debian.org/fixture"',
            f"size = {len(raw)}",
            f'sha256 = "{hashlib.sha256(raw).hexdigest()}"',
            f'members = [{{archive = "{archive_member}", guest = "{guest}"}}]',
        ]
    (source / "config/release_execution_roots.toml").write_text(
        "\n".join(lines) + "\n", encoding="utf-8"
    )
    node = bytes(elf_dynamic_image(b"libatomic.so.1", machine=machine))
    node_arch = "x64" if arch == "x86_64" else "arm64"
    node_root = f"node-v1.0.0-linux-{node_arch}"
    node_url = f"https://nodejs.org/dist/v1.0.0/{node_root}.tar.xz"
    raw = _tar_bytes(f"{node_root}/bin/node", node)
    archives[node_url] = raw
    providers.append(("bin/node", node))
    (source / "config/tool_releases.toml").write_text(
        "\n".join(
            [
                f"schema_version = {tool_releases.TOOL_RELEASES_SCHEMA_VERSION}",
                "[tools.node]",
                'version = "1.0.0"',
                'executable = "node"',
                "[tools.node.provenance]",
                'kind = "checksum-manifest"',
                'url = "https://nodejs.org/dist/v1.0.0/SHASUMS256.txt"',
                f"[tools.node.assets.{arch}-linux]",
                f'url = "{node_url}"',
                f"size = {len(raw)}",
                f'sha256 = "{hashlib.sha256(raw).hexdigest()}"',
                f'archive_member = "{node_root}/bin/node"',
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    return source, archives, dict(providers)


class _ProvisionBody(io.BytesIO):
    status = 200
    headers = {}

    def __init__(self, raw, url):
        super().__init__(raw)
        self.url = url

    def geturl(self):
        return self.url


@pytest.mark.parametrize("arch", ["x86_64", "aarch64"])
def test_explicit_provisioning_populates_exact_cache_then_consumer_is_offline(
    tmp_path, monkeypatch, arch
):
    source, archives, expected = _archive_fixture(tmp_path, arch=arch)
    calls = []

    class Opener:
        def open(self, request, timeout):
            calls.append(request.full_url)
            return _ProvisionBody(archives[request.full_url], request.full_url)

    monkeypatch.setattr(
        tool_releases.urllib.request, "build_opener", lambda *_: Opener()
    )
    cache = tmp_path / "cache"
    admitted = provision_execution_archives.provision(
        target_id=f"linux-{arch}", downloads=cache, source_root=source
    )
    assert calls == list(archives) and len(admitted) == 5
    monkeypatch.setattr(
        tool_releases.urllib.request,
        "build_opener",
        lambda *_: pytest.fail("admitted cache/consumer must not fetch"),
    )
    assert (
        provision_execution_archives.provision(
            target_id=f"linux-{arch}", downloads=cache, source_root=source
        )
        == admitted
    )
    payloads, providers = execution_root.support_payloads(
        cache, arch=arch, source_root=source
    )
    assert payloads == expected and providers == admitted
    assert {
        x["url"]: (cache / x["filename"]).read_bytes() for x in admitted
    } == archives


@pytest.mark.parametrize("arch", ["x86_64", "aarch64"])
@pytest.mark.parametrize("missing", ["libatomic.so.1", "libgcc_s.so.1"])
def test_provisioning_rejects_incomplete_elf_closure_after_exact_cache_admission(
    tmp_path, monkeypatch, arch, missing
):
    source, archives, _ = _archive_fixture(tmp_path, arch=arch, missing=missing)
    calls = []

    class Opener:
        def open(self, request, timeout):
            calls.append(request.full_url)
            return _ProvisionBody(archives[request.full_url], request.full_url)

    monkeypatch.setattr(
        tool_releases.urllib.request, "build_opener", lambda *_: Opener()
    )
    cache = tmp_path / "cache"
    with pytest.raises(ValueError, match="dependency is not admitted") as error:
        provision_execution_archives.provision(
            target_id=f"linux-{arch}", downloads=cache, source_root=source
        )
    assert str(error.value).endswith(" -> " + missing)
    assert calls == list(archives)
    assert {
        url: (cache / url.rsplit("/", 1)[-1]).read_bytes() for url in archives
    } == archives


@pytest.mark.parametrize("arch", ["x86_64", "aarch64"])
def test_payload_and_retained_root_share_lazy_elf_closure(tmp_path, arch):
    source, archives, payloads = _archive_fixture(tmp_path, arch=arch)
    payloads["app/unrelated-data"] = b"not an ELF; outside reachable closure"
    library = f"lib/{arch}-linux-gnu/"
    expected = sorted(
        [
            {"from": "bin/node", "to": library + "libatomic.so.1", "kind": "required"},
            {
                "from": library + "libatomic.so.1",
                "to": library + "libstdc++.so.6",
                "kind": "required",
            },
            {
                "from": library + "libstdc++.so.6",
                "to": library + "libgcc_s.so.1",
                "kind": "required",
            },
            {
                "from": library + "libgcc_s.so.1",
                "to": library + "libc.so.6",
                "kind": "required",
            },
        ],
        key=lambda row: (row["from"], row["to"], row["kind"]),
    )
    assert (
        execution_root.audit_native_payload_closure(
            payloads, arch=arch, executable_paths=["bin/node"]
        )
        == expected
    )
    root = tmp_path / "root"
    rows = [
        execution_root.write_payload(
            root, name, data, executable=name != "app/unrelated-data"
        )
        for name, data in payloads.items()
    ]
    assert (
        execution_root.audit_native_closure(
            root, arch=arch, executable_paths=["bin/node"], expected_files=rows
        )
        == expected
    )


def test_explicit_provision_failure_preserves_verified_entries_and_consumer_refuses_missing(
    tmp_path, monkeypatch
):
    source, archives, _ = _archive_fixture(tmp_path)
    calls = []

    class Opener:
        def open(self, request, timeout):
            calls.append(request.full_url)
            if len(calls) == 2:
                raise TimeoutError("independent interrupted second transfer")
            return _ProvisionBody(archives[request.full_url], request.full_url)

    monkeypatch.setattr(
        tool_releases.urllib.request, "build_opener", lambda *_: Opener()
    )
    cache = tmp_path / "cache"
    with pytest.raises(
        tool_releases.ToolReleaseError, match="independent interrupted second transfer"
    ) as failed:
        provision_execution_archives.provision(
            target_id="linux-x86_64", downloads=cache, source_root=source
        )
    assert isinstance(failed.value.__cause__, TimeoutError)
    first = next(iter(archives))
    assert (cache / first.rsplit("/", 1)[-1]).read_bytes() == archives[first]
    with pytest.raises(tool_releases.ToolReleaseError, match="no automatic fetch"):
        execution_root.support_payloads(cache, arch="x86_64", source_root=source)


@pytest.mark.parametrize(
    "target", ["macos-arm64", "macos-x86_64", "windows-x86_64", "windows-arm64"]
)
def test_platform_without_adapter_fails_preflight_before_any_download(
    tmp_path, monkeypatch, target
):
    monkeypatch.setattr(
        provision_execution_archives,
        "provision_archive",
        lambda **_: pytest.fail("unavailable platform must not download a substitute"),
    )
    with pytest.raises(ValueError, match="no admitted.*filesystem adapter"):
        provision_execution_archives.provision(
            target_id=target, downloads=tmp_path / "cache"
        )
    assert not (tmp_path / "cache").exists()


@pytest.mark.parametrize("kind", ["debian", "node"])
@pytest.mark.parametrize("mutation", ["replace", "overwrite"])
def test_archive_pin_and_payload_parse_share_one_stable_descriptor(
    tmp_path, monkeypatch, kind, mutation
):
    source, archives, _ = _archive_fixture(tmp_path)
    cache = tmp_path / "cache"
    cache.mkdir()
    for url, raw in archives.items():
        (cache / url.rsplit("/", 1)[-1]).write_bytes(raw)
    records = execution_root.archive_inputs(arch="x86_64", source_root=source)
    selected = records[0] if kind == "debian" else records[-1]
    target = cache / selected["filename"]
    real_identity = tool_releases.stable_regular_file_handle_identity

    def capture_then_mutate(opened, **kwargs):
        identity = real_identity(opened, **kwargs)
        if opened.path == target:
            if mutation == "replace":
                changed = tmp_path / "substitution"
                changed.write_bytes(target.read_bytes())
                os.replace(
                    changed, target
                )  # Same bytes, different generation must fail.
            else:
                original_times = target.stat()
                with target.open("r+b") as stream:
                    stream.write(b"changed!")
                    stream.flush()
                    os.fsync(stream.fileno())
                os.utime(
                    target, ns=(original_times.st_atime_ns, original_times.st_mtime_ns)
                )
        return identity

    monkeypatch.setattr(
        tool_releases, "stable_regular_file_handle_identity", capture_then_mutate
    )
    with pytest.raises(
        (ValueError, OSError, tarfile.TarError, tool_releases.ToolReleaseError)
    ):
        execution_root.support_payloads(cache, arch="x86_64", source_root=source)


def test_retained_snapshot_must_match_pin_before_any_payload_derivation(
    tmp_path, monkeypatch
):
    archive = tmp_path / "input"
    archive.mkdir()
    (archive / "provider.deb").write_bytes(b"forged bytes")
    row = {
        "filename": "provider.deb",
        "sha256": hashlib.sha256(b"pinned bytes").hexdigest(),
        "size": len(b"pinned bytes"),
    }
    monkeypatch.setattr(execution_root, "archive_inputs", lambda **_: [row])
    monkeypatch.setattr(
        execution_root,
        "support_payloads",
        lambda *_, **__: pytest.fail(
            "payload derivation must follow retained snapshot pin admission"
        ),
    )
    with pytest.raises(ValueError, match="artifact changed"):
        consumer_replay.prepare(
            evidence=tmp_path / "evidence",
            candidate={"target": {"platform": "linux", "arch": "x86_64"}},
            proofs=[],
            pip_proof={},
            bundle_source=tmp_path,
            archive_cache=archive,
            supervisor=tmp_path / "unused",
            generation={},
            argv=(),
        )


def test_replay_keeps_all_logical_lane_products_distinct():
    from tests.release_lane_fixtures import EXPECTED_LANES, LANE_FIELDS

    cells = [
        {
            "lane": dict(zip(LANE_FIELDS, values, strict=True)),
            "artifact": {"sha256": "a" * 64},
            "manifest": None,
        }
        for values in EXPECTED_LANES
    ]
    bundle = [
        {"python": minor, "cells": copy.deepcopy(cells)}
        for minor in ("3.12", "3.13", "3.14")
    ]
    pip = {"python": "3.12", "cells": copy.deepcopy(cells)}
    rows = consumer_replay.expected_runs(bundle, pip)
    assert len(rows) == len({row["id"] for row in rows}) == 44
    names = {row["id"] for row in rows}
    assert "bundle-3.12-native-native-release-release-size-release" in names
    assert "bundle-3.12-llvm-native-release-release-size-release" in names
    assert "pip-3.12-wasm-wasm-release-wasm-release-release" in names
    native = next(row for row in rows if row["lane"]["backend"] == "llvm")
    assert consumer_replay.guest_command(native, ("argument",)) == [
        f"/app/{native['id']}/program",
        "argument",
    ]


@pytest.mark.parametrize(
    "mutation",
    [
        None,
        "selection-order",
        "empty-selection",
        "duplicate-selection",
        "foreign-selection",
        "untyped-selection",
        "missing-cell",
        "foreign-cell",
        "duplicate-cell",
        "cell-order",
        "default-full",
    ],
)
def test_replay_explicit_selection_requires_its_exact_logical_products(mutation):
    from tests.release_lane_fixtures import EXPECTED_LANES, LANE_FIELDS
    from molt.release_lanes import ReleaseLane

    records = [dict(zip(LANE_FIELDS, EXPECTED_LANES[i], strict=True)) for i in (0, 4)]
    lanes = tuple(ReleaseLane(**record) for record in records)
    selection = lanes
    cells = [{"lane": record, "artifact": {}, "manifest": None} for record in records]
    pip = {"python": "3.12", "cells": copy.deepcopy(cells)}
    if mutation == "selection-order":
        selection = tuple(reversed(lanes))
    elif mutation == "empty-selection":
        selection = ()
    elif mutation == "duplicate-selection":
        selection = (lanes[0], lanes[0])
    elif mutation == "foreign-selection":
        selection = (ReleaseLane(**{**records[0], "backend": "foreign"}),)
    elif mutation == "untyped-selection":
        selection = list(lanes)
    elif mutation == "missing-cell":
        cells.pop()
    elif mutation == "foreign-cell":
        cells[1]["lane"] = dict(zip(LANE_FIELDS, EXPECTED_LANES[8], strict=True))
    elif mutation == "duplicate-cell":
        cells[1] = copy.deepcopy(cells[0])
    elif mutation == "cell-order":
        cells.reverse()
    elif mutation == "default-full":
        selection = None
    kwargs = dict(selected_lanes=selection)
    proofs = [{"python": "3.12", "cells": cells}]
    if mutation not in {None, "selection-order"}:
        with pytest.raises(ValueError, match="release lane selection|consumer replay"):
            consumer_replay.expected_runs(proofs, pip, **kwargs)
        return
    rows = consumer_replay.expected_runs(proofs, pip, **kwargs)
    assert [row["lane"] for row in rows] == records * 2
    assert [row["owner"] for row in rows] == ["bundle", "bundle", "pip", "pip"]
    assert len({row["id"] for row in rows}) == 4


@pytest.mark.parametrize(
    "entrypoint", ["prepare", "validate", "installed", "installed-stdlib"]
)
def test_replay_selection_refuses_before_evidence_or_provider_access(
    tmp_path, monkeypatch, entrypoint
):
    def forbidden(*_args, **_kwargs):
        pytest.fail("invalid lane selection reached an evidence/provider consumer")

    monkeypatch.setattr(execution_root, "archive_inputs", forbidden)
    monkeypatch.setattr(consumer_replay, "_artifact_json", forbidden)
    evidence = tmp_path / "evidence"
    common = dict(candidate={"target": {"platform": "linux", "arch": "x86_64"}})
    selection = (
        {"selected_stdlib_profile": "auto"}
        if entrypoint == "installed-stdlib"
        else {"selected_lanes": ()}
    )
    with pytest.raises(
        ValueError, match="release lane selection|runtime stdlib profile"
    ):
        if entrypoint == "prepare":
            consumer_replay.prepare(
                **common,
                evidence=evidence,
                proofs=[],
                pip_proof={},
                bundle_source=tmp_path,
                archive_cache=tmp_path,
                supervisor=tmp_path / "supervisor",
                generation={},
                argv=(),
                **selection,
            )
        elif entrypoint == "validate":
            consumer_replay.validate(
                {},
                **common,
                evidence_root=evidence,
                proofs=[],
                pip_proof={},
                supervisor=tmp_path / "supervisor",
                argv=(),
                expected_stdout="",
                **selection,
            )
        else:
            consumer_replay.validate_installed_compilation(
                {},
                **common,
                evidence_root=evidence,
                proof={},
                launcher=[],
                controller_python=tmp_path / "python",
                owner="bundle",
                supervisor=tmp_path / "supervisor",
                **selection,
            )
    assert not evidence.exists()


@pytest.mark.parametrize(
    "mutation", ["duplicate", "llvm-target", "unsupported-profile", "extra-field"]
)
def test_replay_refuses_aliases_or_relabelled_logical_products(mutation):
    from tests.release_lane_fixtures import EXPECTED_LANES, LANE_FIELDS

    cells = [
        {
            "lane": dict(zip(LANE_FIELDS, values, strict=True)),
            "artifact": {},
            "manifest": None,
        }
        for values in EXPECTED_LANES
    ]
    pip_cells = copy.deepcopy(cells)
    cell = cells[4]
    if mutation == "duplicate":
        cells.append(copy.deepcopy(cell))
    elif mutation == "llvm-target":
        cell["lane"]["target"] = "llvm"
    elif mutation == "unsupported-profile":
        cell["lane"]["runtime_profile"] = "wasm-release"
    else:
        cell["lane"]["unexpected"] = "claim"
    with pytest.raises(ValueError, match="consumer replay"):
        consumer_replay.expected_runs(
            [{"python": "3.12", "cells": cells}], {"python": "3.12", "cells": pip_cells}
        )


@pytest.mark.parametrize("oversize", [False, True])
def test_replay_stages_only_bounded_compact_supervisor_policy(
    tmp_path, monkeypatch, oversize
):
    # Selection, file staging and sealing are real. Payload providers are
    # fixture inputs; no ELF, compiler, or container claim is made here.
    from tests.release_lane_fixtures import EXPECTED_LANES, LANE_FIELDS
    from molt.release_lanes import ReleaseLane

    lane_record = dict(zip(LANE_FIELDS, EXPECTED_LANES[0], strict=True))
    lane = ReleaseLane(**lane_record)
    program = tmp_path / "program"
    program.write_bytes(b"fixture program, not executed")
    supervisor = tmp_path / "supervisor"
    supervisor.write_bytes(b"fixture supervisor, not executed")
    cell = {
        "lane": lane_record,
        "artifact": {"path": str(program), **execution_root.file_identity(program)},
        "manifest": None,
    }
    monkeypatch.setattr(execution_root, "archive_inputs", lambda **_: [])
    monkeypatch.setattr(execution_root, "support_payloads", lambda *_, **__: ({}, []))
    monkeypatch.setattr(consumer_replay, "wasm_loader_asset_payloads", lambda *_: {})
    closure_calls = []

    def audit_closure(*args, **kwargs):
        closure_calls.append(kwargs["executable_paths"])
        return []

    monkeypatch.setattr(execution_root, "audit_native_closure", audit_closure)
    nonce = "a" * (
        supervisor_custody.SUPERVISOR_BUDGETS["nonce_utf8_bytes"] + 1
        if oversize
        else 64
    )
    monkeypatch.setattr(consumer_replay.secrets, "token_hex", lambda _: nonce)
    evidence = tmp_path / "evidence"
    kwargs = dict(
        evidence=evidence,
        candidate={
            "source_sha": "fixture",
            "target": {"platform": "linux", "arch": "x86_64"},
        },
        proofs=[{"python": "3.12", "cells": [cell]}],
        pip_proof={"python": "3.12", "cells": [copy.deepcopy(cell)]},
        bundle_source=tmp_path,
        archive_cache=tmp_path,
        supervisor=supervisor,
        generation={
            "binary": {
                "sha256": hashlib.sha256(supervisor.read_bytes()).hexdigest(),
                "size_bytes": supervisor.stat().st_size,
            }
        },
        argv=('é\n\\"',),
        selected_lanes=(lane,),
    )
    product_id = "bundle-3.12-native-native-dev-dev-fast-release"
    policy_path = evidence / f"rootfs/policies/{product_id}.json"
    if oversize:
        with pytest.raises(ValueError, match="nonce_utf8_bytes"):
            consumer_replay.prepare(**kwargs)
        assert not policy_path.exists()
        assert not (evidence / "rootfs.tar").exists()
        assert closure_calls == []
        return
    replay = consumer_replay.prepare(**kwargs)
    expected = {
        "schema": supervisor_custody.SUPERVISOR_POLICY_SCHEMA,
        "nonce": nonce,
        "mode": "leaf",
        "cwd": "/app",
        "command": [f"/app/{product_id}/program", *kwargs["argv"]],
        "environment": dict(consumer_replay.ENVIRONMENT),
        "root_role": "guest",
        "fixed_images": [
            {
                "role": "guest",
                "path": f"/app/{product_id}/program",
                "sha256": hashlib.sha256(program.read_bytes()).hexdigest(),
                "root_exit_disposition": "require-exit",
            }
        ],
        "derived_roots": [],
    }
    expected_raw = (
        json.dumps(expected, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
        + "\n"
    ).encode("utf-8")
    assert policy_path.read_bytes() == expected_raw
    retained = next(
        item
        for item in replay["root"]["files"]
        if item["path"] == f"policies/{product_id}.json"
    )
    assert retained["size"] == len(expected_raw)
    assert retained["sha256"] == hashlib.sha256(expected_raw).hexdigest()
    assert {row["id"] for row in replay["runs"]} == {
        product_id,
        "pip-3.12-native-native-dev-dev-fast-release",
    }
    assert len(closure_calls) == 1
    execution_root.validate_sealed_tar(
        evidence / "rootfs.tar",
        evidence / "rootfs",
        expected_archive=replay["root"]["archive"],
    )


@pytest.mark.parametrize("host_platform", ["linux", "darwin", "win32"])
@pytest.mark.parametrize(
    "mutation",
    [
        None,
        "native_custody_valid",
        "journal_coverage_valid",
        "receipt_sha256",
        "policy_input_bytes",
        "guest_platform",
    ],
)
def test_rooted_verifier_binds_guest_receipt_and_requires_native_custody(
    tmp_path, monkeypatch, host_platform, mutation
):
    # The retained guest is Linux on every verifier host. Only execution of the
    # native verifier is replaced; command, raw-byte binding, terminal decoding,
    # and the standalone success consumer remain real.
    rootfs = tmp_path / "rootfs"
    rootfs.mkdir()
    policy = rootfs / "policy.json"
    policy.write_bytes(b' {"fixture":"retained guest policy"}\n')
    receipt = {
        "schema": supervisor_custody.SUPERVISOR_RECEIPT_SCHEMA,
        "capability": {
            "schema": supervisor_custody.SUPERVISOR_CAPABILITY_SCHEMA,
            "platform": "linux",
            "mode": "leaf",
            "backend": "ptrace-exitkill",
            "admission": {
                "state": "admitted",
                "root_stable_process_id": "fixture:guest",
                "root_create_sequence": 1,
                "initial_image_sequence": 2,
            },
            "pre_entry_exec_authority": True,
            "pre_entry_process_create_authority": True,
            "recursive_descendant_authority": True,
            "required_environment": {},
        },
        "complete": True,
        "state": "COMPLETE",
        "root_exit_code": 0,
        "error_count": 0,
        "violation_count": 0,
        "errors": [],
        "violations": [],
        "accounting": {"active_processes": 0},
    }
    if mutation == "guest_platform":
        receipt["capability"]["platform"] = "macos"
    path = tmp_path / "receipt.json"
    raw = ("  " + json.dumps(receipt) + "\n").encode()
    path.write_bytes(raw)
    policy_raw = policy.read_bytes()
    observed = []

    def verify(command, **kwargs):
        assert command == (
            "fixture-supervisor",
            "verify-rooted",
            "--rootfs",
            str(rootfs),
            "--policy",
            str(policy),
            "--receipt",
            str(path),
        )
        assert kwargs["text"] is False
        observed.append(command)
        response = {
            "receipt_sha256": hashlib.sha256(raw).hexdigest(),
            "receipt_bytes": len(raw),
            "policy_input_sha256": hashlib.sha256(policy_raw).hexdigest(),
            "policy_input_bytes": len(policy_raw),
            "native_custody_valid": True,
            "journal_coverage_valid": True,
        }
        if mutation in {"native_custody_valid", "journal_coverage_valid"}:
            response[mutation] = False
        elif mutation == "receipt_sha256":
            response[mutation] = "0" * 64
        elif mutation == "policy_input_bytes":
            response[mutation] = len(policy_raw) + 1
        return subprocess.CompletedProcess(
            command, 0, json.dumps(response).encode("utf-8"), b""
        )

    monkeypatch.setattr(
        supervisor_custody, "sys", SimpleNamespace(platform=host_platform)
    )
    monkeypatch.setattr(supervisor_custody.command_identity, "_run_captured", verify)
    arguments = dict(
        binary=Path("fixture-supervisor"),
        policy_path=policy,
        receipt_path=path,
        rootfs=rootfs,
        cwd=tmp_path,
        env={},
    )
    if mutation is not None:
        with pytest.raises(
            ValueError,
            match="verified different|custody or coverage|schema or mode mismatch|platform",
        ):
            supervisor_custody._validated_supervisor_receipt(**arguments)
    else:
        validated = supervisor_custody._validated_supervisor_receipt(**arguments)
        assert validated == receipt
        consumer_replay.require_success(validated)
    assert len(observed) == 1


def test_compile_root_projection_cannot_be_replayed_as_an_ordinary_guest(tmp_path):
    projection = cross_run.SealedExecutionProjection.compilation(
        memory_max_bytes=512 * 1024 * 1024,
        workspace_max_bytes=128 * 1024 * 1024,
        runtime_max_seconds=90,
    )
    root = tmp_path / "root"
    root.mkdir()
    row = execution_root.write_payload(
        root, "app/input.py", b"print(37)\n", executable=False
    )
    archive = tmp_path / "root.tar"
    seal = execution_root.seal_root(
        root, archive, expected_files=[row], projection=projection
    )
    execution_root.validate_sealed_tar(
        archive, root, expected_archive=seal["archive"], projection=projection
    )
    with pytest.raises(ValueError, match="unadmitted directory"):
        execution_root.validate_sealed_tar(
            archive, root, expected_archive=seal["archive"]
        )
    assert (root / "work").is_dir()
    assert not tuple((root / "work").iterdir())


@pytest.mark.parametrize(
    "defect",
    [
        None,
        "old-cache-pid",
        "worker-substitution",
        "worker-omission",
        "linker-omission",
        "driver-omission",
        "driver-substitution",
        "forged-summary",
        "missing-origin-scope",
        "wrong-nonce-digest",
    ],
)
def test_compilation_process_join_replays_native_receipt_before_origin_join(
    tmp_path, monkeypatch, defect
):
    # These are receiver controls over independently authored journal bytes.
    # Native verification is a seam, so this never claims a kernel positive.
    from molt.backend_protocol import (
        BackendEmissionOrigin,
        BackendEmissionProducer,
        BackendAddressedProgramInput,
        BackendCodegenOrigin,
        BackendNativeWorkerOrigin,
    )
    from molt.cli.link_input_observation import compilation_input_observation

    compiler_image = ("/compiler", "a" * 64, 17)
    linker_image = ("/ld.lld", "b" * 64, 19)
    driver_image = ("/clang", "2" * 64, 23)
    output = {"size": 3, "sha256": hashlib.sha256(b"obj").hexdigest()}
    runtime = {"size": 3, "sha256": hashlib.sha256(b"lib").hexdigest()}
    origin_scope = "b" * 64 if defect == "missing-origin-scope" else "a" * 64
    worker = BackendNativeWorkerOrigin(8, origin_scope, "d" * 64, 5, "e" * 64, 3)
    origin = BackendEmissionOrigin(
        BackendEmissionProducer("daemon", 7, origin_scope),
        BackendAddressedProgramInput("c" * 64, 5, "1" * 64 + "2" * 64),
        output["sha256"],
        output["size"],
        BackendCodegenOrigin(None, (worker,)),
    )
    observation = compilation_input_observation(
        origin.payload(),
        {
            "schema": "molt.link-input-observation.v2",
            "dialect": "elf-gnu",
            "linker_path": "/ld.lld",
            "inputs": [
                {"archive_path": "object", **output},
                {"archive_path": "runtime", **runtime},
            ],
            "roles": {"compiler_output": "object", "runtime": "runtime"},
            "reproducer": {"size": 2048, "sha256": "f" * 64},
        },
        stdlib=None,
        cache={"compiler": None, "stdlib": None},
    )
    records = []
    for pid, image in [
        (7, compiler_image),
        (8, compiler_image),
        (9, linker_image),
        (10, driver_image),
    ]:
        if (
            defect == "old-cache-pid"
            and pid == 7
            or defect == "worker-omission"
            and pid == 8
            or defect == "linker-omission"
            and pid == 9
            or defect == "driver-omission"
            and pid == 10
        ):
            continue
        if defect == "driver-substitution" and pid == 10:
            image = (driver_image[0], "0" * 64, driver_image[2])
        if defect == "worker-substitution" and pid == 8:
            image = ("/other", "0" * 64, 17)
        records.append(
            {
                "sequence": len(records),
                "process_id": pid,
                "stable_process_id": f"linux:{pid}:1",
                "event": {
                    "kind": "exec",
                    "image": {
                        "path": image[0],
                        "sha256": image[1],
                        "size_bytes": image[2],
                    },
                },
            }
        )
    raw = b"".join(json.dumps(row).encode() + b"\n" for row in records)
    digest = hashlib.sha256(raw).hexdigest()
    receipt_path = tmp_path / "receipt.json"
    policy_path = tmp_path / "policy.json"
    policy_bytes = _json({"nonce": "a" * 64})
    policy_path.write_bytes(policy_bytes)
    journal = receipt_path.with_name(f"{receipt_path.name}.events.{digest}.jsonl")
    journal.write_bytes(raw)
    receipt = {
        "complete": True,
        "state": "COMPLETE",
        "capability": {"admission": {"state": "admitted"}},
        "root_exit_code": 0,
        "error_count": 0,
        "violation_count": 0,
        "errors": [],
        "violations": [],
        "accounting": {"active_processes": 0},
        "identity_sha256": "1" * 64,
        "policy_sha256": hashlib.sha256(policy_bytes).hexdigest(),
        "nonce_sha256": (
            "0" * 64
            if defect == "wrong-nonce-digest"
            else hashlib.sha256(("a" * 64).encode()).hexdigest()
        ),
        "event_log": {
            "schema": supervisor_custody.SUPERVISOR_EVENT_LOG_SCHEMA,
            "file": journal.name,
            "bytes": len(raw),
            "count": len(records),
            "sha256": digest,
        },
    }
    invoked = []

    def replay(**kwargs):
        invoked.append(kwargs)
        assert kwargs["require_closed"] is True
        assert kwargs["rootfs"] == tmp_path / "rootfs"
        assert kwargs["policy_path"] == policy_path
        assert kwargs["policy_path"].read_bytes() == policy_bytes
        if defect == "forged-summary":
            raise ValueError("native replay rejected forged summary")
        return receipt

    monkeypatch.setattr(supervisor_custody, "_validated_supervisor_receipt", replay)
    kwargs = dict(
        supervisor=tmp_path / "supervisor",
        rootfs=tmp_path / "rootfs",
        policy_path=tmp_path / "policy.json",
        receipt_path=receipt_path,
        observation=observation,
        compiler_image=compiler_image,
        linker_image=linker_image,
        additional_images=(driver_image,),
    )
    if defect is not None:
        with pytest.raises(ValueError):
            consumer_replay.compilation_process_images(**kwargs)
    else:
        joined = consumer_replay.compilation_process_images(**kwargs)
        assert [
            (row["process_id"], row["stable_process_id"])
            for row in joined["compiler_processes"]
        ] == [(7, "linux:7:1"), (8, "linux:8:1")]
        assert joined["receipt_identity_sha256"] == receipt["identity_sha256"]
        assert all(row["scope"] == "a" * 64 for row in joined["compiler_processes"])
        assert joined["emission_scopes"] == [
            {
                "scope": "a" * 64,
                "policy_sha256": hashlib.sha256(policy_bytes).hexdigest(),
                "receipt_identity_sha256": "1" * 64,
                "event_log": receipt["event_log"],
            }
        ]
    assert len(invoked) == 1


@pytest.mark.parametrize(
    "defect", [None, "missing-binding", "ambiguous-root", "changed-node"]
)
def test_compilation_python_projection_uses_semantic_aliases_and_retained_nodes(defect):
    from molt.python_capture import _file_nodes, PYTHON_CAPTURE_SCHEMA
    from molt.exact_json import canonical_json_sha256
    from tests.python_environment_test_support import (
        runtime_identity_manifest,
        realized_environment_manifest,
    )

    environment = realized_environment_manifest(
        runtime_identity_manifest(), [("example", "1.0")]
    )
    selected = environment["selected_executable"]
    entry = next(
        row for row in environment["tree"]["entries"] if row["path"] == selected["path"]
    )
    alias = selected["path"] + "-alias"
    environment["tree"]["entries"].append({**entry, "path": alias, "kind": "hardlink"})
    environment["tree"]["entries"].sort(
        key=lambda row: (row["path"].casefold(), row["path"])
    )
    environment["tree"]["file_count"] += 1
    environment["tree"]["manifest_sha256"] = canonical_json_sha256(
        environment["tree"]["entries"]
    )
    environment["environment_closure_sha256"] = canonical_json_sha256(
        {k: v for k, v in environment.items() if k != "environment_closure_sha256"}
    )
    by_node = {
        row["node"]: row["path"]
        for row in environment["tree"]["entries"]
        if row["kind"] == "file"
    }
    nodes = sorted(_file_nodes(environment).items())
    entries = []
    for pointer, node in nodes:
        path = (
            "/runtime/os.py"
            if pointer.startswith("/runtime/")
            else "/frontend/" + by_node[node["id"]]
        )
        entries.append(
            (pointer, {"path": path, "size": node["size"], "sha256": node["sha256"]})
        )
    entries.sort(key=lambda item: item[1]["path"])
    capture = {
        "schema": PYTHON_CAPTURE_SCHEMA,
        "identity": environment,
        "file_custody": [row for _, row in entries],
        "node_custody": sorted(
            [
                {"node": pointer, "file_index": index}
                for index, (pointer, _) in enumerate(entries)
            ],
            key=lambda row: row["node"],
        ),
        "inventory_profile": {
            "hash_workers": 1,
            "hashed_files": len(entries),
            "hashed_bytes": len(entries),
            "hash_seconds": 0.0,
        },
    }
    if defect == "missing-binding":
        capture["node_custody"].pop()
    elif defect == "changed-node":
        capture["file_custody"][0]["sha256"] = "f" * 64
    elif defect == "ambiguous-root":
        next(row for row in capture["file_custody"] if row["path"] == "/runtime/os.py")[
            "path"
        ] = "/runtime/unrelated.py"
    if defect:
        with pytest.raises(ValueError):
            execution_root.compilation_python_files(capture)
    else:
        projected = dict(execution_root.compilation_python_files(capture))
        assert (
            projected[Path("/frontend") / alias]
            == projected[Path("/frontend") / selected["path"]]
        )
        assert projected[Path("/runtime/os.py")]["sha256"] == "a" * 64
        assert len(projected) == len(entries) + 1


@pytest.mark.parametrize("metadata", [None, {"comment": "hidden"}, {"mtime": "1"}])
def test_sealed_root_long_runtime_names_use_one_exact_pax_encoding(tmp_path, metadata):
    root = tmp_path / "root"
    name = "runtime/" + "a" * 120 + ".native-link-custody.json"
    row = execution_root.write_payload(
        root, name, b"retained runtime custody", executable=False
    )
    archive = tmp_path / "root.tar"
    seal = execution_root.seal_root(root, archive, expected_files=[row])
    # An independent standard reader must see the exact long semantic name.
    with tarfile.open(archive, "r:") as handle:
        assert handle.extractfile(name).read() == b"retained runtime custody"
    if metadata is None:
        execution_root.validate_sealed_tar(
            archive, root, expected_archive=seal["archive"]
        )
        return
    replacement = tmp_path / "other.tar"
    with (
        tarfile.open(archive, "r:") as source,
        tarfile.open(replacement, "w", format=tarfile.PAX_FORMAT) as target,
    ):
        for member in source:
            if member.name == name:
                member.pax_headers.update(metadata)
            stream = source.extractfile(member) if member.isfile() else None
            target.addfile(member, stream)
    with pytest.raises(ValueError, match="metadata|closure differs"):
        execution_root.validate_sealed_tar(
            replacement,
            root,
            expected_archive=execution_root.file_identity(replacement),
        )


def _compilation_export_fixture(
    tmp_path,
    target,
    defect=None,
    *,
    worker_family=None,
    worker_defect=None,
    windows_coordinates=False,
):
    from molt.backend_protocol import (
        BackendEmissionOrigin,
        BackendEmissionProducer,
        BackendCodegenOrigin,
        BackendAddressedProgramInput,
    )
    from molt.artifact_publication import publication_receipt_path
    from molt.cli.runtime_fingerprints import _fingerprint_payload
    from molt.cli.static_archive_identity import artifact_content_identity
    from tools.release.archive import write_reproducible_zip

    source = tmp_path / "export"
    source.mkdir()
    primary = "binary" if target == "native" else "linked"
    program = source / ("program" if target == "native" else "program.wasm")
    program.write_bytes(b"executable")
    command = ["/installed/molt", "build", "/app/source.py"]
    (source / "command.json").write_text(
        json.dumps({"argv": command, "returncode": 0, "duration_sec": 0.25}),
        encoding="utf-8",
    )
    (source / "command.stdout").write_bytes(b"")
    (source / "command.stderr").write_bytes(b"")

    def content(data):
        return {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}

    emitted, runtime = b"object", b"runtime"
    worker_origins = ()
    stdlib_origin = None
    archive_bytes = None
    if worker_family is not None:
        from molt.backend_protocol import BackendNativeWorkerOrigin, BackendStdlibOrigin
        from tests.native_artifact_fixtures import native_relocatable_object

        assert target == "native" and worker_family in {"app", "stdlib"}
        members = tuple(
            native_relocatable_object(
                target_triple="x86_64-unknown-linux-gnu", symbols=(symbol,)
            )
            for symbol in ("first", "second")
        )
        # Independent ar framing with actual ELF members. Both outputs come
        # from one process, so omission changes neither the process hint set
        # nor the archive bytes/whole digest. No production archive serializer.
        framed = bytearray(b"!<arch>\n")
        for index, member in enumerate(members):
            name = f"molt_{index:08}.o/"
            header = f"{name:<16}{0:<12}{0:<6}{0:<6}{'100644':<8}{len(member):<10}`\n".encode()
            assert len(header) == 60
            framed.extend(header + member)
            if len(member) % 2:
                framed.extend(b"\n")
        archive_bytes = bytes(framed)
        worker_origins = tuple(
            BackendNativeWorkerOrigin(
                8, "a" * 64, "d" * 64, 12, content(member)["sha256"], len(member)
            )
            for member in members
        )
        if worker_family == "app":
            emitted = archive_bytes
        else:
            stdlib_origin = BackendStdlibOrigin(
                content(archive_bytes)["sha256"], len(archive_bytes), worker_origins
            )
    # Independent LLD archive coordinates: drive/UNC provenance cannot be
    # reconstructed by prefixing '/'. Do not call the production projector.
    object_coordinate = "C/work/object" if windows_coordinates else "work/object"
    runtime_coordinate = (
        "D/installed/runtime" if windows_coordinates else "installed/runtime"
    )
    stdlib_coordinate = "C/work/stdlib.a" if windows_coordinates else "work/stdlib.a"
    crt_coordinate = (
        "C/usr/lib/molt-capture-fixture-crt.o"
        if windows_coordinates
        else "usr/lib/molt-capture-fixture-crt.o"
    )
    buffers = {object_coordinate: emitted, runtime_coordinate: runtime}
    if worker_family == "stdlib":
        buffers[stdlib_coordinate] = archive_bytes
    if target == "native":
        buffers[crt_coordinate] = b"CRT input from the linker buffer"
    report = source / (program.name + ".molt-link-inputs.tar")
    with tarfile.open(report, "w") as archive:
        for name, data in {
            "link/response.txt": (
                b"C:/work/object D:/installed/runtime"
                if windows_coordinates
                else b"/work/object /installed/runtime"
            ),
            "link/version.txt": b"LLD",
            **{"link/" + name: data for name, data in buffers.items()},
        }.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
    observed = {
        "schema": "molt.link-input-observation.v2",
        "dialect": "elf-gnu" if target == "native" else "wasm",
        "linker_path": (
            r"C:\tools\ld.lld.exe" if target == "native" else r"C:\tools\wasm-ld.exe"
        )
        if windows_coordinates
        else ("/ld.lld" if target == "native" else "/wasm-ld"),
        "inputs": [
            {"archive_path": name, **content(data)}
            for name, data in sorted(buffers.items())
        ],
        "roles": {
            "compiler_output"
            if target == "native"
            else "compiler_rewritten": object_coordinate,
            "runtime": runtime_coordinate,
        },
        "reproducer": content(report.read_bytes()),
    }
    if worker_family == "stdlib":
        observed["roles"]["stdlib"] = stdlib_coordinate
    if defect == "forged-summary":
        observed["inputs"][0]["sha256"] = "f" * 64
    link = (
        observed
        if target == "native"
        else {
            "schema": "molt.wasm-link-inputs.v2",
            "source_inputs": {
                "compiler_output": content(emitted),
                "runtime": content(runtime),
                "deployment_runtime": None,
            },
            "links": {"linked": observed},
        }
    )
    origin = BackendEmissionOrigin(
        BackendEmissionProducer("daemon", 7, "a" * 64),
        BackendAddressedProgramInput("a" * 64, 12, "b" * 64),
        content(emitted)["sha256"],
        len(emitted),
        BackendCodegenOrigin(None, worker_origins)
        if worker_family == "app"
        else BackendCodegenOrigin(7, ()),
    )
    compiler_payload = origin.payload()
    stdlib_payload = None if stdlib_origin is None else stdlib_origin.payload()
    if worker_defect is not None:
        workers = (
            compiler_payload["codegen"]["workers"]
            if worker_family == "app"
            else stdlib_payload["workers"]
        )
        original_hints = {(worker["scope"], worker["process_id"]) for worker in workers}
        if worker_defect == "worker-digest":
            workers[0]["output"]["sha256"] = "f" * 64
        elif worker_defect == "worker-omission":
            workers.pop()
        else:
            raise AssertionError(worker_defect)
        assert {
            (worker["scope"], worker["process_id"]) for worker in workers
        } == original_hints
    observation = source / (program.name + ".molt-link-inputs.json")
    observation.write_text(
        json.dumps(
            {
                "schema": "molt.compilation-inputs.v3",
                "cache": {"compiler": None, "stdlib": None},
                "stdlib": stdlib_payload,
                "compiler": compiler_payload,
                "link": link,
            }
        ),
        encoding="utf-8",
    )
    outputs = {
        primary: program,
        "input_observation": observation,
        "input_reproducer" if target == "native" else "linked_input_reproducer": report,
    }
    if target == "native":
        driver_host = r"C:\tools\clang.exe" if windows_coordinates else "/clang"
        stub_host = (
            r"C:\work\main_stub.c" if windows_coordinates else "/work/main_stub.c"
        )
        dependency = source / "program.molt-native-inputs.d"
        dependency.write_text(
            (
                r"output: C\:/work/main_stub.c"
                if windows_coordinates
                else "output: /work/main_stub.c"
            )
            + "\n",
            encoding="utf-8",
        )
        preparation = source / "program.molt-native-inputs.json"
        preparation.write_text(
            json.dumps(
                {
                    "schema": "molt.native-link-preparation.v2",
                    "driver": {
                        "path": driver_host,
                        "content_path": driver_host,
                        **content(b"driver"),
                    },
                    "source": stub_host,
                    "files": [
                        {
                            "path": stub_host,
                            "content_path": stub_host,
                            **content(b"stub"),
                        }
                    ],
                }
            ),
            encoding="utf-8",
        )
        outputs.update(driver_dependencies=dependency, driver_preparation=preparation)
    receipt = {
        "schema": "molt.final-link.v2",
        "fingerprint": _fingerprint_payload({"hash": "a" * 64}),
        "outputs": {
            role: {
                "path": "/work/output/" + path.name,
                "identity": artifact_content_identity(path),
            }
            for role, path in outputs.items()
        },
    }
    if defect == "escaped-output":
        receipt["outputs"][primary]["path"] = "/work/output/../elsewhere"
    publication_receipt_path(program).write_text(json.dumps(receipt), encoding="utf-8")
    if defect == "output-substitution":
        program.write_bytes(b"substitute")
    elif defect == "omitted-report":
        report.unlink()
    archive = tmp_path / "output.zip"
    write_reproducible_zip(source, archive, source_date_epoch=315532800)
    runtime_cell = {
        "files": [
            {
                "role": "runtime_archive"
                if target == "native"
                else "wasm_reloc_member",
                **content(b"changed" if defect == "runtime-substitution" else runtime),
            }
        ]
    }
    projection = cross_run.SealedExecutionProjection.compilation(
        memory_max_bytes=512 * 1024 * 1024,
        workspace_max_bytes=128 * 1024 * 1024,
        runtime_max_seconds=90,
    )
    args = dict(
        archive=archive,
        output=tmp_path / "received",
        projection=projection,
        command=["/other"] if defect == "wrong-command" else command,
        runtime_cell=runtime_cell,
        target=target,
    )
    return args, content, emitted


@pytest.mark.parametrize("worker_family", ["app", "stdlib"])
@pytest.mark.parametrize("worker_defect", [None, "worker-digest", "worker-omission"])
def test_compilation_export_rechecks_worker_archive_members(
    tmp_path, worker_family, worker_defect
):
    args, _, _ = _compilation_export_fixture(
        tmp_path, "native", worker_family=worker_family, worker_defect=worker_defect
    )
    if worker_defect is None:
        received = consumer_replay.receive_compilation_outputs(**args)
        origin = (
            received["observation"]["compiler"]["codegen"]
            if worker_family == "app"
            else received["observation"]["stdlib"]
        )
        assert len(origin["workers"]) == 2
    else:
        message = (
            "member differs" if worker_defect == "worker-digest" else "omits or repeats"
        )
        with pytest.raises(ValueError, match=message):
            consumer_replay.receive_compilation_outputs(**args)


@pytest.mark.parametrize("target", ["native", "wasm"])
@pytest.mark.parametrize(
    "defect",
    [
        None,
        "runtime-substitution",
        "output-substitution",
        "forged-summary",
        "omitted-report",
        "wrong-command",
        "escaped-output",
    ],
)
def test_compilation_export_rechecks_raw_buffers_and_publication(
    tmp_path, target, defect
):
    args, content, emitted = _compilation_export_fixture(tmp_path, target, defect)
    archive = args["archive"]
    if defect:
        with pytest.raises((ValueError, OSError)):
            consumer_replay.receive_compilation_outputs(**args)
    else:
        received = consumer_replay.receive_compilation_outputs(**args)
        assert (
            received["artifact"]["sha256"] == hashlib.sha256(b"executable").hexdigest()
        )
        assert received["observation"]["compiler"]["output"] == content(emitted)
        assert received["observation"]["compiler"]["input"] == {
            "schema": "molt.addressed-program-input.v1",
            "lease": {"sha256": "a" * 64, "size": 12},
            "function_digests": "b" * 64,
        }
        assert received["archive"] == content(archive.read_bytes())


@pytest.mark.parametrize(
    "defect",
    [
        None,
        "duplicate",
        "controller",
        "cell-omission",
        "policy",
        "foreign-guard",
        "launcher",
        "interpreter",
    ],
)
def test_compilation_preparation_shares_input_bytes_and_keeps_cell_policies(
    tmp_path, monkeypatch, defect
):
    duplicate = defect == "duplicate"
    # This tests the preparation/output boundary only. The capture materializers
    # are explicit fixture seams; no installed/kernel execution is claimed.
    from molt.compiler_distribution import InstalledCompiler
    from molt.release_lanes import capture_release_lanes
    from molt.exact_json import read_exact
    from tools.release.release_model import ROOT

    inventory = capture_release_lanes(ROOT)
    native = next(
        lane
        for lane in inventory.lanes
        if lane.target == "native" and lane.backend != "llvm"
    )
    wasm = next(lane for lane in inventory.lanes if lane.target == "wasm")
    compiler = InstalledCompiler(
        Path("/installed/source"),
        "a" * 40,
        {"platform": "linux", "arch": "x86_64", "path": "bin/compiler"},
        {},
        {},
        (),
        {},
    )
    monkeypatch.setattr(consumer_replay, "capture_release_lanes", lambda _: inventory)
    staged = []

    def inputs(root, **kwargs):
        staged.append("inputs")
        execution_root.write_payload(
            root, "installed/bin/compiler", b"compiler", executable=True
        )
        execution_root.write_payload(
            root, "python/bin/python", b"interpreter", executable=True
        )
        execution_root.write_payload(
            root, "sdk/input", b"one shared SDK", executable=False
        )
        return execution_root.root_inventory(root)

    def runtime(root, **kwargs):
        staged.append(kwargs["key"]["cell"])
        return [], {
            "id": kwargs["key"]["cell"],
            "key": {
                "target_triple": "x86_64-unknown-linux-gnu"
                if kwargs["kind"] == "native"
                else "wasm32-wasip1"
            },
            "files": [],
        }

    monkeypatch.setattr(execution_root, "stage_compilation_inputs", inputs)
    monkeypatch.setattr(execution_root, "stage_compilation_runtime", runtime)
    monkeypatch.setattr(execution_root, "support_payloads", lambda *a, **k: ({}, []))
    supervisor = tmp_path / "supervisor"
    supervisor.write_bytes(b"supervisor")
    generation = {
        "binary": {
            "size_bytes": 10,
            "sha256": hashlib.sha256(b"supervisor").hexdigest(),
        }
    }
    projection = cross_run.SealedExecutionProjection.compilation(
        memory_max_bytes=512 * 1024 * 1024,
        workspace_max_bytes=128 * 1024 * 1024,
        runtime_max_seconds=90,
    )
    cells = [
        (native, "native", {"cell": native.id}),
        (native if duplicate else wasm, "wasm", {"cell": wasm.id}),
    ]
    args = dict(
        evidence=tmp_path / "evidence",
        compiler=compiler,
        launcher=["/installed/bin/molt"],
        python_executable=Path("/python/bin/python"),
        python_captures=[{}],
        toolchain_capture={},
        cas_root=tmp_path / "cas",
        native_link_inputs={native.id: (tmp_path / "native-inputs.tar", {}, {})},
        environment={
            "MOLT_HOME": "/installed-home",
            "PATH": "/sdk/bin",
            "MOLT_MEMORY_GUARD_TOKEN": "foreign-token",
        },
        cells=cells,
        python_minor="3.12",
        projection=projection,
        archive_cache=tmp_path / "archives",
        supervisor=supervisor,
        generation=generation,
    )
    if duplicate:
        with pytest.raises(ValueError, match="no applicable"):
            consumer_replay.prepare_compilation(**args)
        assert not staged
        return
    monkeypatch.setattr(
        execution_root, "stage_compilation_link_inputs", lambda *args, **kwargs: []
    )
    monkeypatch.setattr(
        execution_root, "stage_compilation_driver_inputs", lambda *args, **kwargs: []
    )
    prepared = consumer_replay.prepare_compilation(**args)
    assert staged == ["inputs", native.id, wasm.id]
    assert prepared["schema"] == "molt.installed-compilation.v3"
    assert prepared["origins"] == []
    assert prepared["cache_seed"] == []
    assert [row["id"] for row in prepared["cells"]] == [native.id, wasm.id]
    root = args["evidence"] / "rootfs"
    assert (
        len([row for row in prepared["root"]["files"] if row["path"] == "sdk/input"])
        == 1
    )
    policies = []
    for lane, cell in zip((native, wasm), prepared["cells"], strict=True):
        policy = read_exact(
            root / cell["policy"], max_bytes=65536, label="fixture compilation policy"
        )
        config = read_exact(
            root / policy["command"][-1].removeprefix("/"),
            max_bytes=65536,
            label="fixture compilation controller",
        )
        policies.append(policy)
        assert config["command"] == cell["command"]
        assert config["cache_seed"] == "/app/cache/" + lane.id
        assert config["cache_contract"] == {
            "kind": "native-archive" if lane.target == "native" else "wasm",
            "target_triple": "x86_64-unknown-linux-gnu"
            if lane.target == "native"
            else "wasm32-wasip1",
        }
        assert config["compiler_source_root"] == "/installed/source"
        assert config["environment"]["MOLT_EMISSION_SCOPE"] == policy["nonce"]
        assert len(policy["nonce"]) == 64
        assert config["environment"]["MOLT_WASM_CARGO_PROFILE"] == lane.runtime_profile
        assert "MOLT_MEMORY_GUARD_TOKEN" not in config["environment"]
        assert policy["derived_roots"] == []
        assert any(
            row["path"] == "/installed/bin/compiler"
            and row["root_exit_disposition"] == "terminate"
            for row in policy["fixed_images"]
        )
        assert (
            "--capture-link-inputs"
            if lane.target == "native"
            else "--build-arg=--capture-link-inputs"
        ) in config["command"]
    assert policies[0]["nonce"] != policies[1]["nonce"]
    execution_root.validate_sealed_tar(
        args["evidence"] / "rootfs.tar",
        root,
        expected_archive=prepared["root"]["archive"],
        projection=projection,
    )

    if defect == "launcher":
        prepared["launcher"] = ["/python/bin/python", "/app/forged.py"]
    elif defect == "interpreter":
        prepared["controller_python"] = "/other/python"
    elif defect == "controller":
        controller = root / "app/compilation-controller.py"
        controller.chmod(0o644)
        controller.write_bytes(b"X" * controller.stat().st_size)
    elif defect == "cell-omission":
        prepared["cells"].pop()
    elif defect in {"policy", "foreign-guard"}:
        cell = prepared["cells"][0]
        selected = root / (
            cell["policy"]
            if defect == "policy"
            else "app/compilation-" + cell["id"] + ".json"
        )
        value = json.loads(selected.read_text(encoding="utf-8"))
        if defect == "policy":
            value["fixed_images"][0]["sha256"] = "0" * 64
        else:
            value["environment"]["MOLT_MEMORY_GUARD_TOKEN"] = "foreign-token"
        selected.chmod(0o644)
        selected.write_text(json.dumps(value), encoding="utf-8")
    if defect is not None:
        with pytest.raises(ValueError):
            consumer_replay._validate_compilation_plan(
                rootfs=root,
                compilation=prepared,
                lanes=(native, wasm),
                python_minor="3.12",
                launcher=args["launcher"],
                controller_python=args["python_executable"],
            )
    else:
        consumer_replay._validate_compilation_plan(
            rootfs=root,
            compilation=prepared,
            lanes=(native, wasm),
            python_minor="3.12",
            launcher=args["launcher"],
            controller_python=args["python_executable"],
        )


@pytest.mark.parametrize(
    "defect",
    [
        None,
        "forged-output-summary",
        "forged-process-summary",
        "retained-output-substitution",
    ],
)
def test_retained_compilation_replays_export_before_accepting_summary(
    tmp_path, monkeypatch, defect
):
    # Namespace/native admission are independently tested seams here. This
    # exercises retained readback, the real ZIP/raw-link receiver, publication
    # identities and final equality; it does not simulate a kernel positive.
    args, _, _ = _compilation_export_fixture(tmp_path, "native")
    evidence = tmp_path / "retained"
    (evidence / "transport").mkdir(parents=True)
    archive = evidence / "transport/abc.stdout"
    archive.write_bytes(args["archive"].read_bytes())
    rootfs = evidence / "rootfs"
    rootfs.mkdir()
    compiler_path = rootfs / "compiler"
    compiler_path.write_bytes(b"compiler")
    (rootfs / "ld.lld").write_bytes(b"linker")
    (rootfs / "clang").write_bytes(b"driver")
    lane = consumer_replay.capture_release_lanes(consumer_replay.ROOT).lanes[0]
    assert lane.target == "native"
    cell = {
        "id": lane.id,
        "lane": lane.as_record(),
        "command": args["command"],
        "runtime_cell": args["runtime_cell"],
        "policy": "policies/cell.json",
        "execution": {
            "execution": args["projection"].to_dict(),
            "observation": {},
            "nonce": "abc",
            "teardown": {},
            "stdout": "transport/abc.stdout",
            "stderr": "transport/abc.stderr",
            "receipt": f"runs/{lane.id}/receipt.json",
        },
    }
    installed = SimpleNamespace(
        binary=compiler_path,
        runtime={"cells": [args["runtime_cell"]]},
        verify_binary=lambda *args: None,
    )
    monkeypatch.setattr(
        consumer_replay, "_validate_compilation_inputs", lambda **kwargs: installed
    )
    monkeypatch.setattr(
        consumer_replay, "_validate_transport", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        consumer_replay,
        "validate_sealed_systemd_configuration",
        lambda *args, **kwargs: {"InvocationID": "abc", "Id": "unit"},
    )
    monkeypatch.setattr(
        consumer_replay,
        "validate_sealed_systemd_terminal",
        lambda *args, **kwargs: None,
    )
    monkeypatch.setattr(
        consumer_replay, "validate_sealed_systemd_stop", lambda *args, **kwargs: None
    )
    policy_path = rootfs / cell["policy"]
    policy_path.parent.mkdir(parents=True)
    policy_path.write_text(json.dumps({"nonce": "a" * 64}), encoding="utf-8")
    scope_sources = {"a" * 64: (policy_path, evidence / cell["execution"]["receipt"])}
    verified_scopes = {}
    scope_checks = []

    def native_scope(scope, **kwargs):
        # This fixture controls native admission; the separate process-join
        # family replays real journal bytes and rejects absent original scopes.
        assert scope == "a" * 64
        assert kwargs["sources"] == scope_sources
        scope_checks.append(scope)
        result = ({"identity_sha256": "a" * 64}, None)
        kwargs["verified_scopes"][scope] = result
        return result

    monkeypatch.setattr(consumer_replay, "_verified_emission_scope", native_scope)
    process_calls = []

    def native_join(**kwargs):
        assert kwargs["additional_images"] == (
            ("/clang", hashlib.sha256(b"driver").hexdigest(), len(b"driver")),
        )
        assert kwargs["_scope_sources"] == scope_sources
        process_calls.append(kwargs)
        return {"receipt_identity_sha256": "a" * 64}

    monkeypatch.setattr(consumer_replay, "compilation_process_images", native_join)
    compilation = {
        name: None
        for name in (
            "schema",
            "execution",
            "target",
            "source_sha",
            "cells",
            "python",
            "source_sha256",
            "launcher",
            "controller_python",
            "compiler",
            "compiler_source_root",
            "environment_contract",
            "providers",
            "root",
            "supervisor_generation",
            "origins",
            "cache_seed",
            "transport",
            "files",
        )
    }
    compilation.update(
        schema="molt.installed-compilation.v3",
        origins=[],
        cache_seed=[],
        execution=args["projection"].to_dict(),
        target={"arch": "x86_64"},
        transport={"root": "/root"},
        cells=[cell],
    )
    consumer_replay._receive_compilation_cell(
        evidence=evidence,
        rootfs=rootfs,
        compilation=compilation,
        cell=cell,
        installed=installed,
        supervisor=tmp_path / "supervisor",
        scope_sources=scope_sources,
        verified_scopes=verified_scopes,
    )
    assert cell["outputs"]["artifact"]["path"] == "program"
    assert len(process_calls) == 1
    if defect == "forged-output-summary":
        cell["outputs"]["artifact"]["sha256"] = "0" * 64
    elif defect == "forged-process-summary":
        cell["processes"]["receipt_identity_sha256"] = "0" * 64
    elif defect == "retained-output-substitution":
        (evidence / "outputs" / lane.id / "program").write_bytes(b"substitute")
    # A forged summary may recompute this outer inventory. Raw export readback
    # must still reject altered retained bytes and derived claims.
    compilation["files"] = consumer_replay._evidence_inventory(evidence)
    (evidence / "compilation.json").write_text(
        json.dumps(compilation), encoding="utf-8"
    )
    kwargs = dict(
        evidence=evidence,
        compilation=compilation,
        compiler=installed,
        lanes=(lane,),
        python_minor="3.12",
        supervisor=tmp_path / "supervisor",
        launcher=["/installed/molt"],
        controller_python=Path("/python"),
    )
    if defect:
        with pytest.raises(ValueError, match="summary differs|output differs"):
            consumer_replay.validate_compilation(**kwargs)
    else:
        consumer_replay.validate_compilation(**kwargs)
    assert len(process_calls) == 2
    assert scope_checks == ["a" * 64]


@pytest.mark.parametrize(
    "defect", [None, "conflicting-root-input", "buffer-substitution", "forged-summary"]
)
def test_compilation_native_resources_come_from_captured_buffers(tmp_path, defect):
    _compilation_export_fixture(tmp_path, "native")
    source = tmp_path / "export"
    report = source / "program.molt-link-inputs.tar"
    observation = json.loads(
        (source / "program.molt-link-inputs.json").read_text(encoding="utf-8")
    )["link"]
    root = tmp_path / "rootfs"
    root.mkdir()
    crt = "usr/lib/molt-capture-fixture-crt.o"
    if defect == "conflicting-root-input":
        execution_root.write_payload(
            root, crt, b"X" * len(b"CRT input from the linker buffer"), executable=False
        )
    elif defect == "buffer-substitution":
        with tarfile.open(report) as archive:
            offset = archive.getmember("link/" + crt).offset_data
        raw = bytearray(report.read_bytes())
        raw[offset] ^= 1
        report.write_bytes(raw)
    elif defect == "forged-summary":
        next(row for row in observation["inputs"] if row["archive_path"] == crt)[
            "sha256"
        ] = "0" * 64
    kwargs = dict(root=root, reproducer=report, observation=observation)
    if defect:
        with pytest.raises(ValueError):
            execution_root.stage_compilation_link_inputs(**kwargs)
        assert not (tmp_path / "root.tar").exists()
        if defect == "conflicting-root-input":
            assert (root / crt).read_bytes().startswith(b"XXXX")
    else:
        rows = execution_root.stage_compilation_link_inputs(**kwargs)
        assert [row["path"] for row in rows] == [crt]
        assert (root / crt).read_bytes() == b"CRT input from the linker buffer"
        assert not (root / "work/object").exists()
        assert not (root / "installed/runtime").exists()
        projection = cross_run.SealedExecutionProjection.compilation(
            memory_max_bytes=512 * 1024 * 1024,
            workspace_max_bytes=128 * 1024 * 1024,
            runtime_max_seconds=90,
        )
        sealed = execution_root.seal_root(
            root, tmp_path / "root.tar", expected_files=rows, projection=projection
        )
        execution_root.validate_sealed_tar(
            tmp_path / "root.tar",
            root,
            expected_archive=sealed["archive"],
            projection=projection,
        )


@pytest.mark.parametrize(
    "defect", [None, "header-substitution", "driver-substitution", "omitted-source"]
)
def test_native_preparation_stages_lexical_header_inputs_without_readmitting_changes(
    tmp_path, defect
):
    from molt.cli.link_input_observation import capture_native_link_preparation

    source = tmp_path / "main.c"
    source.write_bytes(b"#include <selected.h>\n")
    header = tmp_path / "content.h"
    header.write_bytes(b"#define VALUE 1\n")
    alias = tmp_path / "selected.h"
    alias.symlink_to(header.name)
    driver = tmp_path / "clang"
    driver.write_bytes(b"selected driver")
    dependencies = tmp_path / "driver.d"
    dependencies.write_text(
        "program: "
        + ("" if defect == "omitted-source" else "main.c ")
        + "selected.h\n",
        encoding="utf-8",
    )
    if defect == "omitted-source":
        with pytest.raises(ValueError, match="omitted its source"):
            capture_native_link_preparation(
                dependencies, source=source, driver=driver, cwd=tmp_path
            )
        return
    captured = capture_native_link_preparation(
        dependencies, source=source, driver=driver, cwd=tmp_path
    )
    if os.name == "nt":
        with pytest.raises(ValueError, match="Linux host coordinate"):
            execution_root.stage_compilation_driver_inputs(
                tmp_path / "unsupported-root", preparation=captured
            )
        assert not (tmp_path / "unsupported-root").exists()
        return
    row = next(row for row in captured["files"] if row["path"] == str(alias))
    assert row["content_path"] == str(header)
    root = tmp_path / "rootfs"
    root.mkdir()
    execution_root.stage_file(
        root, str(driver).removeprefix("/"), driver, executable=True
    )
    if defect == "header-substitution":
        header.write_bytes(b"#define VALUE 9\n")
    elif defect == "driver-substitution":
        captured["driver"]["sha256"] = hashlib.sha256(b"replaced driver").hexdigest()
    if defect:
        with pytest.raises(
            ValueError, match="changed before staging|conflicts with an admitted"
        ):
            execution_root.stage_compilation_driver_inputs(root, preparation=captured)
    else:
        retained = execution_root.stage_compilation_driver_inputs(
            root, preparation=captured
        )
        staged_alias = root / str(alias).removeprefix("/")
        assert staged_alias.is_file() and not staged_alias.is_symlink()
        assert staged_alias.read_bytes() == b"#define VALUE 1\n"
        assert not (root / str(source).removeprefix("/")).exists()
        assert {row["path"] for row in retained} == {str(alias).removeprefix("/")}


def test_repeated_warm_cache_selection_keeps_only_the_original_emission_scope(tmp_path):
    # Selection is not native admission: independently authored source sessions
    # remain raw artifacts, and later validation must replay their journals.
    _compilation_export_fixture(tmp_path, "native")
    observation = json.loads(
        (tmp_path / "export/program.molt-link-inputs.json").read_bytes()
    )
    evidence = tmp_path / "prior"
    rootfs = evidence / "rootfs"
    policy_path = rootfs / "policies/cell.json"
    config_path = rootfs / "app/compilation-cell.json"
    policy_path.parent.mkdir(parents=True)
    config_path.parent.mkdir(parents=True)
    cold_identity, warm_identity = "d" * 64, "e" * 64
    cold_dir = evidence / "origins" / cold_identity
    old_warm_dir = evidence / "origins" / warm_identity
    for directory, scope, identity in (
        (cold_dir, "a" * 64, cold_identity),
        (old_warm_dir, "b" * 64, warm_identity),
    ):
        directory.mkdir(parents=True)
        (directory / "policy.json").write_text(
            json.dumps({"nonce": scope}), encoding="utf-8"
        )
        (directory / "config.json").write_text(
            json.dumps({"environment": {"MOLT_EMISSION_SCOPE": scope}}),
            encoding="utf-8",
        )
        (directory / "receipt.json").write_text(
            json.dumps({"identity_sha256": identity}), encoding="utf-8"
        )
    cells = [
        {
            "id": "cell",
            "policy": "policies/cell.json",
            "outputs": {"observation": observation},
        }
    ]
    original_bytes = {path.name: path.read_bytes() for path in cold_dir.iterdir()}
    for current_scope, origins in (
        (
            "b" * 64,
            [{"scope": "a" * 64, "cell": "cell", "identity_sha256": cold_identity}],
        ),
        (
            "c" * 64,
            [
                {"scope": "a" * 64, "cell": "cell", "identity_sha256": cold_identity},
                {"scope": "b" * 64, "cell": "cell", "identity_sha256": warm_identity},
            ],
        ),
    ):
        policy_path.write_text(json.dumps({"nonce": current_scope}), encoding="utf-8")
        config_path.write_text(
            json.dumps({"environment": {"MOLT_EMISSION_SCOPE": current_scope}}),
            encoding="utf-8",
        )
        assert consumer_replay._compilation_reuse_scopes(
            evidence, {"cells": cells, "origins": origins}
        ) == [("a" * 64, "cell", cold_dir / "policy.json", cold_dir / "receipt.json")]
    assert {
        path.name: path.read_bytes() for path in cold_dir.iterdir()
    } == original_bytes


@pytest.mark.parametrize("target", ["native", "wasm"])
def test_compilation_export_preserves_foreign_host_archive_coordinates(
    tmp_path, target
):
    # This is a raw retained-byte replay on any test host, not Windows sealed
    # execution. Literal LLD members deliberately omit their DOS drive colons.
    args, content, emitted = _compilation_export_fixture(
        tmp_path, target, windows_coordinates=True
    )
    received = consumer_replay.receive_compilation_outputs(**args)
    link = received["observation"]["link"]
    if target == "wasm":
        assert link["schema"] == "molt.wasm-link-inputs.v2"
        link = link["links"]["linked"]
    assert link["schema"] == "molt.link-input-observation.v2"
    assert link["linker_path"] == (
        r"C:\tools\ld.lld.exe" if target == "native" else r"C:\tools\wasm-ld.exe"
    )
    expected = {
        "C/work/object": content(emitted),
        "D/installed/runtime": content(b"runtime"),
    }
    if target == "native":
        expected["C/usr/lib/molt-capture-fixture-crt.o"] = content(
            b"CRT input from the linker buffer"
        )
        assert received["preparation"]["driver"]["path"] == r"C:\tools\clang.exe"
        assert received["preparation"]["source"] == r"C:\work\main_stub.c"
    assert {
        row["archive_path"]: {"size": row["size"], "sha256": row["sha256"]}
        for row in link["inputs"]
    } == expected
    assert link["roles"] == {
        "compiler_output"
        if target == "native"
        else "compiler_rewritten": "C/work/object",
        "runtime": "D/installed/runtime",
    }


@pytest.mark.parametrize("projection", ["link", "driver"])
def test_compilation_staging_rejects_foreign_host_before_writing(tmp_path, projection):
    _compilation_export_fixture(tmp_path, "native", windows_coordinates=True)
    source = tmp_path / "export"
    root = tmp_path / "rootfs"
    root.mkdir()
    if projection == "link":
        observation = json.loads(
            (source / "program.molt-link-inputs.json").read_text(encoding="utf-8")
        )["link"]
        with pytest.raises(ValueError, match="Linux host coordinate"):
            execution_root.stage_compilation_link_inputs(
                root,
                reproducer=source / "program.molt-link-inputs.tar",
                observation=observation,
            )
    else:
        preparation = json.loads(
            (source / "program.molt-native-inputs.json").read_text(encoding="utf-8")
        )
        with pytest.raises(ValueError, match="Linux host coordinate"):
            execution_root.stage_compilation_driver_inputs(
                root, preparation=preparation
            )
    assert list(root.iterdir()) == []


@pytest.mark.parametrize(
    "coordinate",
    [
        r"C:\tools\ld.lld.exe",
        r"\\server\share\ld.lld.exe",
        "C:/tools/ld.lld.exe",
        "usr/lib/a.o",
        "/usr/../lib/a.o",
        "/usr//lib/a.o",
        "/usr/./lib/a.o",
        "/",
    ],
)
def test_linux_root_projection_rejects_foreign_or_noncanonical_coordinates(coordinate):
    with pytest.raises(ValueError):
        execution_root.linux_root_relative_path(coordinate)


def test_linux_root_projection_preserves_explicit_namespace():
    assert (
        execution_root.linux_root_relative_path("/usr/lib/a.o").as_posix()
        == "usr/lib/a.o"
    )
    assert (
        execution_root.linux_root_relative_path("/work/output/program").as_posix()
        == "work/output/program"
    )
