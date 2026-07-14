from __future__ import annotations

import base64
import hashlib
import json
import zipfile
from pathlib import Path

import pytest

from molt.cli.extension_wheel import (
    ExtensionWheelError,
    _rewrite_staged_extension_wheel,
    _write_extension_wheel,
)


def _source_wheel(path: Path, *, raw_root: Path) -> None:
    extension_path = "scipy/ndimage/_nd_image.molt.wasm"
    manifest = {
        "module": "scipy.ndimage._nd_image",
        "extension": extension_path,
        "wheel": path.name,
        "sources": [str(raw_root / "nd_image.c")],
    }
    _write_extension_wheel(
        path,
        entries=(
            (extension_path, b"\x00asm-canonical"),
            (
                "extension_manifest.json",
                json.dumps(manifest, sort_keys=True).encode() + b"\n",
            ),
            ("scipy-1.0.dist-info/WHEEL", b"Wheel-Version: 1.0\n"),
            ("scipy-1.0.dist-info/METADATA", b"Metadata-Version: 2.1\n"),
        ),
        record_path="scipy-1.0.dist-info/RECORD",
    )


def _record_line(path: str, data: bytes) -> str:
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).decode()
    return f"{path},sha256={digest.rstrip('=')},{len(data)}"


def test_staged_wheel_identity_comes_only_from_canonical_manifest(tmp_path: Path) -> None:
    filename = "scipy-1.0-py3-molt_abi1-wasm32_wasip1.whl"
    first_source = tmp_path / "first" / filename
    second_source = tmp_path / "second" / filename
    _source_wheel(first_source, raw_root=tmp_path / "short")
    _source_wheel(
        second_source,
        raw_root=tmp_path / "a-host-build-root-with-a-different-length",
    )
    canonical_sidecar = {
        "module": "scipy.ndimage._nd_image",
        "extension": "_nd_image.molt.wasm",
        "wheel": "../../../provenance/wheels/scipy/ndimage/_nd_image/" + filename,
        "wheel_sha256": "raw-wheel-self-reference-must-not-survive",
        "extension_sha256": hashlib.sha256(b"\x00asm-canonical").hexdigest(),
        "generated_at_utc": "1970-01-01T00:00:00Z",
        "sources": ["@source/scipy/ndimage/src/nd_image.c"],
    }
    first_destination = tmp_path / "published-a" / filename
    second_destination = tmp_path / "published-b" / filename

    first_sha, first_embedded = _rewrite_staged_extension_wheel(
        first_source,
        first_destination,
        canonical_embedded_manifest=canonical_sidecar,
    )
    second_sha, second_embedded = _rewrite_staged_extension_wheel(
        second_source,
        second_destination,
        canonical_embedded_manifest=canonical_sidecar,
    )

    assert first_sha == second_sha
    assert first_destination.read_bytes() == second_destination.read_bytes()
    assert first_embedded == second_embedded
    assert "extension_sha256" in first_embedded
    assert not ({"wheel_sha256", "generated_at_utc"} & first_embedded.keys())
    assert str(tmp_path) not in json.dumps(first_embedded)
    with zipfile.ZipFile(first_destination) as wheel:
        infos = wheel.infolist()
        assert [info.filename for info in infos] == sorted(
            info.filename for info in infos[:-1]
        ) + ["scipy-1.0.dist-info/RECORD"]
        assert all(info.date_time == (1980, 1, 1, 0, 0, 0) for info in infos)
        record_lines = wheel.read("scipy-1.0.dist-info/RECORD").decode().splitlines()
        for info in infos[:-1]:
            assert _record_line(info.filename, wheel.read(info)) in record_lines
        assert record_lines[-1] == "scipy-1.0.dist-info/RECORD,,"


@pytest.mark.parametrize(
    ("entries", "match"),
    (
        (
            (
                ("extension_manifest.json", b"{}"),
                ("extension_manifest.json", b"{}"),
            ),
            "duplicate wheel member",
        ),
        ((("pkg/native.molt.wasm", b"wasm"),), "missing extension_manifest"),
        (
            (
                ("extension_manifest.json", b'{"extension":"pkg/native.molt.wasm"}'),
                ("pkg/native.molt.wasm", b"wasm"),
                ("pkg-1.0.dist-info/WHEEL", b"wheel"),
                ("pkg-1.0.dist-info/METADATA", b"metadata"),
                ("pkg-1.0.dist-info/RECORD", b"record"),
            ),
            "must not provide RECORD",
        ),
    ),
)
def test_wheel_writer_rejects_ambiguous_authority(
    tmp_path: Path,
    entries: tuple[tuple[str, bytes], ...],
    match: str,
) -> None:
    with pytest.raises(ExtensionWheelError, match=match):
        _write_extension_wheel(
            tmp_path / "invalid.whl",
            entries=entries,
            record_path="pkg-1.0.dist-info/RECORD",
        )
