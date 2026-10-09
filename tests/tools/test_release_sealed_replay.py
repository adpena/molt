"""Independent byte/protocol rejection oracles; no Docker/compiled-guest claims."""

from __future__ import annotations

import copy
import hashlib
import io
import os
from pathlib import Path
import tarfile
import json

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
        "DockerTransport",
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


def _image():
    return {
        "Id": "sha256:" + "a" * 64,
        "Os": "linux",
        "Architecture": "amd64",
        "RootFS": {"Type": "layers", "Layers": ["sha256:" + "b" * 64]},
        "Config": {},
    }


@pytest.mark.parametrize(
    "mutate",
    [
        lambda x: x.update(Architecture="arm64"),
        lambda x: x["RootFS"].update(Layers=["sha256:" + "c" * 64]),
        lambda x: x["RootFS"]["Layers"].append("sha256:" + "d" * 64),
        lambda x: x["Config"].update(Env=["LD_PRELOAD=/host/libpython.so"]),
        lambda x: x["Config"].update(Volumes={"/host": {}}),
    ],
)
def test_imported_image_binds_exact_uncompressed_tar_and_has_no_inherited_inputs(
    mutate,
):
    value = _image()
    cross_run.validate_sealed_docker_image(value, sha256="b" * 64, arch="x86_64")
    mutate(value)
    with pytest.raises(ValueError):
        cross_run.validate_sealed_docker_image(value, sha256="b" * 64, arch="x86_64")


def _container():
    command = ["/bin/molt-proof-supervisor", "run-export"]
    return command, {
        "Image": "sha256:" + "a" * 64,
        "Path": command[0],
        "Args": command[1:],
        "Config": {
            "Entrypoint": command[:1],
            "Cmd": command[1:],
            "WorkingDir": "/app",
            "User": "65534:65534",
            "Env": ["PATH=/absent", "HOME=/absent", "LANG=C", "LC_ALL=C"],
            "Hostname": "molt-guest",
            "Tty": False,
            "OpenStdin": False,
        },
        "HostConfig": {
            "NetworkMode": "none",
            "ReadonlyRootfs": True,
            "Privileged": False,
            "CapDrop": ["ALL"],
            "SecurityOpt": ["no-new-privileges"],
            "PidMode": "",
            "IpcMode": "private",
            "UTSMode": "",
            "UsernsMode": "",
            "CgroupnsMode": "private",
            "Runtime": "runc",
            "ShmSize": 16777216,
            "LogConfig": {"Type": "none", "Config": {}},
            "Dns": ["127.0.0.1"],
            "DnsSearch": ["."],
            "DnsOptions": ["ndots:0"],
            "Tmpfs": {
                "/tmp": "rw,noexec,nosuid,nodev,size=64m,mode=1777",
                "/evidence": "rw,noexec,nosuid,nodev,size=16m,mode=1777",
            },
            "Memory": 1073741824,
            "MemorySwap": 1073741824,
            "PidsLimit": 64,
            "NanoCpus": 1000000000,
            "AutoRemove": False,
            "RestartPolicy": {"Name": "no"},
            "ReadonlyPaths": [
                "/proc/bus",
                "/proc/fs",
                "/proc/irq",
                "/proc/sys",
                "/proc/sysrq-trigger",
            ],
            "MaskedPaths": ["/proc/kcore", "/sys/firmware"],
        },
        "State": {"Running": False},
        "Mounts": [],
        "NetworkSettings": {"Networks": {"none": {}}},
    }


@pytest.mark.parametrize(
    "section,key,value",
    [
        ("HostConfig", "NetworkMode", "host"),
        ("HostConfig", "PidMode", "host"),
        ("HostConfig", "Runtime", "unknown-runtime"),
        ("HostConfig", "ReadonlyRootfs", False),
        ("HostConfig", "Privileged", True),
        ("HostConfig", "Binds", ["/usr:/host:ro"]),
        ("HostConfig", "Devices", [{"PathOnHost": "/dev/sda"}]),
        ("HostConfig", "ReadonlyPaths", []),
        ("Config", "Env", ["PATH=/usr/bin", "PYTHONPATH=/host"]),
        ("Config", "Entrypoint", ["/bin/python3"]),
    ],
)
def test_effective_container_isolation_rejects_positive_host_contamination(
    section, key, value
):
    command, observed = _container()
    cross_run.validate_sealed_docker_configuration(
        observed, image=observed["Image"], command=command
    )
    observed[section][key] = value
    with pytest.raises(ValueError):
        cross_run.validate_sealed_docker_configuration(
            observed, image=observed["Image"], command=command
        )


def test_effective_mounts_cannot_hide_socket_or_host_bind():
    command, observed = _container()
    observed["Mounts"] = [
        {
            "Type": "bind",
            "Source": "/var/run/docker.sock",
            "Destination": "/tmp/control",
        }
    ]
    with pytest.raises(ValueError, match="host mount"):
        cross_run.validate_sealed_docker_configuration(
            observed, image=observed["Image"], command=command
        )


def test_success_gate_rejects_failed_or_incomplete_terminal():
    # Exact native verifier invocation is exercised by the owning custody
    # tests. This independently rejects superficially successful failed receipts.
    valid = {
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
        ("root_exit_code", 1),
        ("error_count", 1),
        ("violation_count", 1),
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
                "schema_version = 2",
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
    with pytest.raises(TimeoutError):
        provision_execution_archives.provision(
            target_id="linux-x86_64", downloads=cache, source_root=source
        )
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
