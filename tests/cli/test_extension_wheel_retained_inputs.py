from __future__ import annotations

import base64
import csv
import hashlib
import io
import json
import zipfile
from pathlib import Path
from typing import Any

import pytest

from molt.cli.extension_wheel import (
    ExtensionWheelError,
    _rewrite_staged_extension_wheel,
    _write_extension_wheel,
)
from molt.cli.source_extension_input_custody import (
    source_extension_input_custody_manifest,
    source_extension_input_custody_path,
)


def _source_wheel(
    path: Path,
    *,
    extra_entries: tuple[tuple[str, bytes], ...] = (),
    manifest_fields: dict[str, Any] | None = None,
) -> dict[str, object]:
    extension = b"fixture-extension"
    manifest: dict[str, object] = {
        "extension": "pkg/module.molt.wasm",
        "extension_sha256": hashlib.sha256(extension).hexdigest(),
        "wheel": path.name,
        "runtime_python_import_modules": ["pkg.helper"],
    }
    manifest.update(manifest_fields or {})
    _write_extension_wheel(
        path,
        entries=(
            (str(manifest["extension"]), extension),
            ("extension_manifest.json", json.dumps(manifest).encode()),
            ("pkg-1.0.dist-info/WHEEL", b"Wheel-Version: 1.0\n"),
            ("pkg-1.0.dist-info/METADATA", b"Metadata-Version: 2.1\n"),
            *extra_entries,
        ),
        record_path="pkg-1.0.dist-info/RECORD",
    )
    return manifest


def _custodied_inputs() -> tuple[dict[str, Any], dict[str, bytes]]:
    source = b"int PyInit_module(void);\n"
    header = b"#define VALUE 1\n"
    source_sha256 = hashlib.sha256(source).hexdigest()
    header_sha256 = hashlib.sha256(header).hexdigest()
    source_ref = source_extension_input_custody_path(source_sha256).as_posix()
    header_ref = source_extension_input_custody_path(header_sha256).as_posix()
    return (
        {
            "input_custody": source_extension_input_custody_manifest(),
            "sources": [source_ref],
            "object_closure": {
                "objects": [
                    {
                        "source": source_ref,
                        "language": "c",
                        "source_sha256": source_sha256,
                        "dependencies": [{"path": header_ref, "sha256": header_sha256}],
                    }
                ]
            },
        },
        {source_ref: source, header_ref: header},
    )


def test_write_validates_complete_retained_input_bytes(tmp_path: Path) -> None:
    manifest, inputs = _custodied_inputs()
    wheel = tmp_path / "custodied.whl"

    _source_wheel(wheel, extra_entries=tuple(inputs.items()), manifest_fields=manifest)

    with zipfile.ZipFile(wheel) as archive:
        for member, data in inputs.items():
            assert archive.read(member) == data


@pytest.mark.parametrize(
    ("fault", "diagnostic"),
    [
        ("missing_source", "missing retained input"),
        ("missing_header", "missing retained input"),
        ("changed_source", "retained input checksum mismatch"),
        ("changed_header", "retained input checksum mismatch"),
        ("null_descriptor", "invalid wheel input custody"),
        ("wrong_descriptor", "invalid wheel input custody"),
        ("nonportable_source", "invalid wheel input custody"),
        ("wrong_digest_path", "invalid wheel input custody"),
        ("escaping_source", "invalid wheel member path"),
        ("malformed_header_ref", "invalid wheel input custody"),
    ],
)
def test_write_rejects_incomplete_or_invalid_input_custody_before_publication(
    tmp_path: Path, fault: str, diagnostic: str
) -> None:
    manifest, inputs = _custodied_inputs()
    source = manifest["object_closure"]["objects"][0]
    header = source["dependencies"][0]
    if fault.startswith(("missing_", "changed_")):
        member = source["source"] if fault.endswith("source") else header["path"]
        if fault.startswith("missing_"):
            del inputs[member]
        else:
            inputs[member] = b"changed after staging"
    elif fault == "null_descriptor":
        manifest["input_custody"] = None
    elif fault == "wrong_descriptor":
        manifest["input_custody"]["algorithm"] = "md5"
    elif fault == "malformed_header_ref":
        del source["dependencies"]
        source["dependencies_ref"] = "missing-reference"
    else:
        if fault == "nonportable_source":
            source["source"] = source["source"].replace("/", "\\")
        elif fault == "wrong_digest_path":
            source["source"] = source_extension_input_custody_path("1" * 64).as_posix()
        else:
            source["source"] = "../" + source["source"]
        manifest["sources"] = [source["source"]]
    destination = tmp_path / "custodied.whl"
    destination.write_bytes(b"previous published wheel")

    with pytest.raises(ExtensionWheelError, match=diagnostic):
        _source_wheel(
            destination,
            extra_entries=tuple(inputs.items()),
            manifest_fields=manifest,
        )

    assert destination.read_bytes() == b"previous published wheel"


@pytest.mark.parametrize("already_embedded", [False, True])
@pytest.mark.parametrize(
    "fault", [None, "changed_header", "missing_header", "null_descriptor"]
)
def test_rewrite_cannot_bypass_retained_input_validation(
    tmp_path: Path, already_embedded: bool, fault: str | None
) -> None:
    custody, inputs = _custodied_inputs()
    header = custody["object_closure"]["objects"][0]["dependencies"][0]["path"]
    if fault == "changed_header":
        inputs[header] = b"changed after staging"
    elif fault == "missing_header":
        del inputs[header]
    elif fault == "null_descriptor":
        custody["input_custody"] = None
    original = tmp_path / "original.whl"
    canonical = _source_wheel(
        original, extra_entries=tuple(inputs.items()) if already_embedded else ()
    )
    canonical.update(custody)
    retained_inputs: dict[str, Path] = {}
    if not already_embedded:
        for index, (member, data) in enumerate(inputs.items()):
            source = tmp_path / f"retained-{index}"
            source.write_bytes(data)
            retained_inputs[member] = source
    destination = tmp_path / "rewritten.whl"
    destination.write_bytes(b"previous published wheel")

    if fault is None:
        _rewrite_staged_extension_wheel(
            original,
            destination,
            canonical_embedded_manifest=canonical,
            retained_inputs=retained_inputs,
        )
        with zipfile.ZipFile(destination) as archive:
            for member, data in inputs.items():
                assert archive.read(member) == data
    else:
        diagnostic = {
            "changed_header": "retained input checksum mismatch",
            "missing_header": "missing retained input",
            "null_descriptor": "invalid wheel input custody",
        }[fault]
        with pytest.raises(ExtensionWheelError, match=diagnostic):
            _rewrite_staged_extension_wheel(
                original,
                destination,
                canonical_embedded_manifest=canonical,
                retained_inputs=retained_inputs,
            )
        assert destination.read_bytes() == b"previous published wheel"


@pytest.mark.parametrize("already_embedded", [False, True])
def test_rewrite_retains_inputs_once_and_rebuilds_complete_record(
    tmp_path: Path, already_embedded: bool
) -> None:
    source = tmp_path / "source.c"
    source.write_bytes(b'PyObject *p = PyImport_ImportModule("pkg.helper");\n')
    data = source.read_bytes()
    member = source_extension_input_custody_path(
        hashlib.sha256(data).hexdigest()
    ).as_posix()
    original = tmp_path / "original.whl"
    canonical = _source_wheel(
        original, extra_entries=((member, data),) if already_embedded else ()
    )
    canonical.update(
        extension="module.molt.wasm",
        generated_at_utc="2026-09-05T00:00:00Z",
        wheel_sha256="stale-sidecar-digest",
    )
    destination = tmp_path / "staged.whl"

    digest, embedded = _rewrite_staged_extension_wheel(
        original,
        destination,
        canonical_embedded_manifest=canonical,
        retained_inputs={member: source},
    )

    assert digest == hashlib.sha256(destination.read_bytes()).hexdigest()
    assert embedded["wheel"] == destination.name
    assert embedded["runtime_python_import_modules"] == ["pkg.helper"]
    assert "wheel_sha256" not in embedded
    assert "generated_at_utc" not in embedded
    assert canonical["wheel_sha256"] == "stale-sidecar-digest"
    with zipfile.ZipFile(destination) as archive:
        names = archive.namelist()
        assert names.count(member) == 1
        assert archive.read(member) == data
        assert "pkg/module.molt.wasm" not in names
        assert archive.read("module.molt.wasm") == b"fixture-extension"
        assert json.loads(archive.read("extension_manifest.json")) == embedded
        record_path = "pkg-1.0.dist-info/RECORD"
        rows = list(csv.reader(io.StringIO(archive.read(record_path).decode("utf-8"))))
        assert len(rows) == len(names) == len(set(names))
        assert {row[0] for row in rows} == set(names)
        for path, checksum, size in rows:
            if path == record_path:
                assert (checksum, size) == ("", "")
                continue
            payload = archive.read(path)
            expected = base64.urlsafe_b64encode(hashlib.sha256(payload).digest())
            assert checksum == "sha256=" + expected.rstrip(b"=").decode("ascii")
            assert size == str(len(payload))
        assert all(
            info.date_time == (1980, 1, 1, 0, 0, 0) for info in archive.infolist()
        )


def test_conflicting_retained_member_preserves_existing_destination(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source.c"
    source.write_bytes(b"new source")
    member = source_extension_input_custody_path(
        hashlib.sha256(source.read_bytes()).hexdigest()
    ).as_posix()
    original = tmp_path / "original.whl"
    canonical = _source_wheel(original, extra_entries=((member, b"different bytes"),))
    destination = tmp_path / "staged.whl"
    destination.write_bytes(b"previous published wheel")

    with pytest.raises(ExtensionWheelError, match="retained source input conflicts"):
        _rewrite_staged_extension_wheel(
            original,
            destination,
            canonical_embedded_manifest=canonical,
            retained_inputs={member: source},
        )

    assert destination.read_bytes() == b"previous published wheel"


def test_missing_retained_input_preserves_existing_destination(tmp_path: Path) -> None:
    original = tmp_path / "original.whl"
    canonical = _source_wheel(original)
    destination = tmp_path / "staged.whl"
    destination.write_bytes(b"previous published wheel")
    member = source_extension_input_custody_path("1" * 64).as_posix()

    with pytest.raises(OSError):
        _rewrite_staged_extension_wheel(
            original,
            destination,
            canonical_embedded_manifest=canonical,
            retained_inputs={member: tmp_path / "missing.c"},
        )

    assert destination.read_bytes() == b"previous published wheel"
