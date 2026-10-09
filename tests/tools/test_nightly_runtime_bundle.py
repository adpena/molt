"""The Nightly bundle transports exactly the toolchain the CLI exports.

Round trips use real checkouts (``tests/cli/native_toolchain_test_support``):
the producer's build publishes a runtime generation and admits a backend, and
the consumer's own build admission is the oracle for what an import
delivered. Transport faults must be refused before the CLI publishes anything.
"""

from __future__ import annotations

import io
import json
from pathlib import Path
import subprocess
import tarfile

import pytest

from molt.cli import backend_binary
from molt.cli import native_toolchain_transfer as transfer
from molt.cli import runtime_native_build
from molt.exact_json import encode_exact
from tests.cli.native_toolchain_test_support import (
    BACKEND_FEATURES,
    BACKEND_PROFILE,
    RUNTIME_PROFILE,
    Checkout,
    checkout,
    runtime_identity,
)
from tests.runtime_build_identity_helper import native_runtime_staticlib_identity
from tools import nightly_runtime_bundle as bundle

COMMIT = "1" * 40
IDENTITY = bundle.BundleIdentity.from_runtime(COMMIT, runtime_identity())


def _pack(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[Path, dict]:
    producer = checkout(tmp_path, monkeypatch, "producer")
    producer.build(monkeypatch)
    archive = tmp_path / "runtime-bundle.tar"
    manifest = bundle.pack_bundle(
        members=transfer.export_native_toolchain(producer.selection),
        output=archive,
        manifest_output=tmp_path / "runtime-bundle-manifest.json",
        identity=IDENTITY,
        runtime_build_identity=runtime_identity(),
    )
    return archive, manifest


def _consumer(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, **seeds) -> Checkout:
    consumer = checkout(tmp_path, monkeypatch, "consumer", **seeds)
    consumer.activate(monkeypatch)
    return consumer


def _extract(archive: Path, consumer: Checkout, **expected):
    return bundle.verify_extract_bundle(
        archive=archive,
        selection=consumer.selection,
        expected_identity=expected.get("identity", IDENTITY),
        expected_runtime_build_identity=expected.get(
            "runtime_build_identity", runtime_identity()
        ),
    )


def _published_nothing(consumer: Checkout) -> bool:
    return not (consumer.root / "target").exists()


def _rewrite(archive: Path, edit) -> None:
    """Rewrite the archive member list through ``edit(name, info, data)``."""
    with tarfile.open(archive, mode="r:") as source:
        members = [
            (info, source.extractfile(info).read() if info.isreg() else b"")
            for info in source.getmembers()
        ]
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.USTAR_FORMAT) as out:
        for info, data in members:
            for new_info, new_data in edit(info, data):
                out.addfile(
                    new_info, io.BytesIO(new_data) if new_info.isreg() else None
                )
    archive.write_bytes(buffer.getvalue())


def _manifest_rewrite(archive: Path, change) -> None:
    def edit(info, data):
        if info.name != bundle.MANIFEST_NAME:
            return [(info, data)]
        payload = json.loads(data)
        change(payload)
        encoded = encode_exact(payload)
        info.size = len(encoded)
        return [(info, encoded)]

    _rewrite(archive, edit)


def test_round_trip_delivers_what_the_consumer_build_admits_without_cargo(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, manifest = _pack(tmp_path, monkeypatch)
    consumer = _consumer(tmp_path, monkeypatch)

    assert _extract(archive, consumer) == manifest

    selection = consumer.selection
    candidates = runtime_native_build._native_runtime_generation_candidates(
        selection.runtime_lib,
        project_root=consumer.root,
        cargo_profile=RUNTIME_PROFILE,
        target_triple=None,
    )
    assert [generation.build_identity for generation in candidates] == [
        runtime_identity()
    ]

    def cargo(*_args, **_kwargs):
        raise AssertionError("a bundled backend must not be rebuilt")

    monkeypatch.setattr(backend_binary, "_run_resolved_cargo_plan", cargo)
    monkeypatch.setattr(
        backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **_kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )
    assert backend_binary._ensure_backend_binary(
        selection.backend.binary,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile=BACKEND_PROFILE,
        project_root=consumer.root,
        backend_features=BACKEND_FEATURES,
    )


def test_manifest_records_each_exported_member_with_its_mode(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _archive, manifest = _pack(tmp_path, monkeypatch)

    assert [(record["role"], record["mode"]) for record in manifest["files"]] == [
        (transfer.RUNTIME_SELECTION_ROLE, "0644"),
        (transfer.RUNTIME_ARCHIVE_ROLE, "0644"),
        (transfer.NATIVE_LINK_MANIFEST_ROLE, "0644"),
        (transfer.BACKEND_EXECUTABLE_ROLE, "0755"),
        (transfer.BACKEND_RECEIPT_ROLE, "0644"),
    ]
    assert manifest["identity"] == IDENTITY.as_dict()


def test_pack_is_byte_deterministic(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, _manifest = _pack(tmp_path, monkeypatch)
    first = archive.read_bytes()
    producer = checkout(tmp_path, monkeypatch, "producer")
    producer.activate(monkeypatch)
    bundle.pack_bundle(
        members=transfer.export_native_toolchain(producer.selection),
        output=archive,
        manifest_output=tmp_path / "runtime-bundle-manifest.json",
        identity=IDENTITY,
        runtime_build_identity=runtime_identity(),
    )
    assert archive.read_bytes() == first


def test_pack_rejects_executable_bits_that_disagree_with_the_role(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    producer = checkout(tmp_path, monkeypatch, "producer")
    producer.build(monkeypatch)
    members = transfer.export_native_toolchain(producer.selection)
    members[1].path.chmod(0o755)

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="executable bits"):
        bundle.pack_bundle(
            members=members,
            output=tmp_path / "runtime-bundle.tar",
            manifest_output=tmp_path / "runtime-bundle-manifest.json",
            identity=IDENTITY,
            runtime_build_identity=runtime_identity(),
        )
    assert not (tmp_path / "runtime-bundle.tar").exists()


def test_pack_rejects_oversized_payload_before_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(bundle, "_MAX_BUNDLE_PAYLOAD_BYTES", 64)
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="safety limit"):
        _pack(tmp_path, monkeypatch)
    assert not (tmp_path / "runtime-bundle.tar").exists()


def test_extract_rejects_member_bytes_that_differ_from_the_manifest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, _manifest = _pack(tmp_path, monkeypatch)

    def edit(info, data):
        if info.name.startswith(transfer.BACKEND_EXECUTABLE_ROLE + "/"):
            data = data[:-1] + b"X"
        return [(info, data)]

    _rewrite(archive, edit)
    consumer = _consumer(tmp_path, monkeypatch)
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="hash"):
        _extract(archive, consumer)
    assert _published_nothing(consumer)


def test_extract_rejects_a_tar_mode_the_manifest_does_not_attest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, _manifest = _pack(tmp_path, monkeypatch)

    def edit(info, data):
        if info.name.startswith(transfer.RUNTIME_ARCHIVE_ROLE + "/"):
            info.mode = 0o755
        return [(info, data)]

    _rewrite(archive, edit)
    consumer = _consumer(tmp_path, monkeypatch)
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="mode"):
        _extract(archive, consumer)
    assert _published_nothing(consumer)


@pytest.mark.parametrize(
    "kind", ["symlink", "directory", "duplicate", "extra", "missing"]
)
def test_extract_rejects_a_member_closure_the_manifest_does_not_declare(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kind: str
) -> None:
    archive, _manifest = _pack(tmp_path, monkeypatch)

    def edit(info, data):
        if not info.name.startswith(transfer.BACKEND_RECEIPT_ROLE + "/"):
            return [(info, data)]
        if kind == "symlink":
            link = tarfile.TarInfo(info.name)
            link.type = tarfile.SYMTYPE
            link.linkname = "/etc/passwd"
            return [(link, b"")]
        if kind == "directory":
            directory = tarfile.TarInfo(info.name)
            directory.type = tarfile.DIRTYPE
            return [(directory, b"")]
        if kind == "duplicate":
            return [(info, data), (info, data)]
        if kind == "extra":
            extra = tarfile.TarInfo("extra/payload")
            extra.size = len(data)
            return [(info, data), (extra, data)]
        return []

    _rewrite(archive, edit)
    consumer = _consumer(tmp_path, monkeypatch)
    with pytest.raises(bundle.NightlyRuntimeBundleError):
        _extract(archive, consumer)
    assert _published_nothing(consumer)


@pytest.mark.parametrize(
    "change,match",
    [
        (lambda payload: payload["files"][0].update(role="unknown"), "unknown"),
        (lambda payload: payload["files"][0].update(name="../escape"), "portable"),
        (lambda payload: payload["files"][3].update(mode="0777"), "mode"),
        (lambda payload: payload["files"].pop(4), "missing roles"),
        (lambda payload: payload.update(schema_version=3), "schema"),
        (lambda payload: payload.update(extra=True), "shape"),
    ],
)
def test_extract_rejects_an_invalid_manifest_before_staging(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change, match: str
) -> None:
    archive, _manifest = _pack(tmp_path, monkeypatch)
    _manifest_rewrite(archive, change)
    consumer = _consumer(tmp_path, monkeypatch)
    with pytest.raises(bundle.NightlyRuntimeBundleError, match=match):
        _extract(archive, consumer)
    assert _published_nothing(consumer)


def test_extract_rejects_another_source_commit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, _manifest = _pack(tmp_path, monkeypatch)
    consumer = _consumer(tmp_path, monkeypatch)
    other = bundle.BundleIdentity("2" * 40, IDENTITY.target_triple)
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="source or target"):
        _extract(archive, consumer, identity=other)
    assert _published_nothing(consumer)


def test_extract_rejects_another_runtime_build_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, _manifest = _pack(tmp_path, monkeypatch)
    consumer = _consumer(tmp_path, monkeypatch)
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="runtime build"):
        _extract(archive, consumer, runtime_build_identity=runtime_identity("changed"))
    assert _published_nothing(consumer)


def test_extract_reports_the_cli_refusal_of_a_foreign_backend(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, _manifest = _pack(tmp_path, monkeypatch)
    consumer = _consumer(tmp_path, monkeypatch, backend_seed="changed")
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="CLI refused"):
        _extract(archive, consumer)
    assert not (consumer.root / "target" / BACKEND_PROFILE).exists()


def test_extract_rejects_an_oversized_archive_before_hashing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, _manifest = _pack(tmp_path, monkeypatch)
    monkeypatch.setattr(bundle, "_MAX_BUNDLE_ARCHIVE_BYTES", 1024)
    consumer = _consumer(tmp_path, monkeypatch)
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="safety limit"):
        _extract(archive, consumer)
    assert _published_nothing(consumer)


@pytest.mark.parametrize("target", list(bundle._NATIVE_TARGET_CELLS))
def test_bundle_platform_is_derived_from_captured_runtime_target_without_host_probe(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, target: str
) -> None:
    build_identity = native_runtime_staticlib_identity(
        cargo_profile=RUNTIME_PROFILE, host_target=target
    )
    monkeypatch.setattr(
        bundle,
        "_run_identity_command",
        lambda command, **_kwargs: COMMIT if "rev-parse" in command else "",
    )
    identity = bundle.collect_bundle_identity(
        tmp_path, runtime_build_identity=build_identity
    )
    assert identity.target_triple == target
    assert (
        identity.platform_system,
        identity.platform_machine,
    ) == bundle._NATIVE_TARGET_CELLS[target]
    assert bundle._validated_identity(identity.as_dict()) == identity


def test_collect_bundle_identity_refuses_a_dirty_checkout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        bundle,
        "_run_identity_command",
        lambda command, **_kwargs: COMMIT if "rev-parse" in command else " M x.py",
    )
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="dirty"):
        bundle.collect_bundle_identity(
            tmp_path, runtime_build_identity=runtime_identity()
        )


@pytest.mark.parametrize(
    "target_triple",
    [
        "x86_64-unknown-freebsd",
        "riscv64gc-unknown-linux-gnu",
        "x86_64-unknown-linux-musl",
    ],
)
def test_bundle_identity_rejects_unsupported_target_cells(target_triple: str) -> None:
    with pytest.raises(ValueError, match="unsupported.*target"):
        bundle.BundleIdentity(source_commit=COMMIT, target_triple=target_triple)


def test_bundle_rejects_internally_consistent_but_foreign_target_projection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _archive, manifest = _pack(tmp_path, monkeypatch)
    foreign = bundle.BundleIdentity(COMMIT, "aarch64-apple-darwin")
    manifest["identity"] = foreign.as_dict()
    with pytest.raises(
        bundle.NightlyRuntimeBundleError, match="captured runtime effective target"
    ):
        bundle.validate_manifest(
            manifest,
            expected_identity=foreign,
            expected_runtime_build_identity=runtime_identity(),
        )


def test_bundle_never_labels_musl_runtime_as_gnu() -> None:
    build_identity = native_runtime_staticlib_identity(
        cargo_profile=RUNTIME_PROFILE, host_target="x86_64-unknown-linux-musl"
    )
    with pytest.raises(ValueError, match="unsupported.*target"):
        bundle.BundleIdentity.from_runtime(COMMIT, build_identity)


@pytest.mark.parametrize(
    "constant", ["NaN", "Infinity", "-Infinity", "1e9999", "-1e9999"]
)
def test_manifest_uses_exact_json_for_nonfinite_values(constant: str) -> None:
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="non-finite JSON"):
        bundle._read_manifest_bytes(('{"extra": ' + constant + "}").encode())


def test_manifest_rejects_duplicate_keys() -> None:
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="invalid"):
        bundle._read_manifest_bytes(b'{"kind": 1, "kind": 2}')


@pytest.mark.parametrize("field", ["system", "machine"])
@pytest.mark.parametrize("value", [None, False, 1, [], {}])
def test_bundle_identity_never_coerces_nonstring_platform_values(
    field: str, value: object
) -> None:
    identity = IDENTITY.as_dict()
    identity["platform"][field] = value
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="non-empty strings"):
        bundle._validated_identity(identity)


@pytest.mark.parametrize("version", [True, 4.0])
def test_bundle_rejects_noninteger_schema_version(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, version: object
) -> None:
    _archive, payload = _pack(tmp_path, monkeypatch)
    payload["schema_version"] = version
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="schema is unsupported"):
        bundle.validate_manifest(
            payload,
            expected_identity=IDENTITY,
            expected_runtime_build_identity=runtime_identity(),
        )
