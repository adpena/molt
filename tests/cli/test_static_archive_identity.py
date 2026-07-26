from __future__ import annotations

from pathlib import Path

import pytest

from molt.cli.static_archive_identity import (
    StaticArchiveIdentityError,
    artifact_content_identity,
    static_archive_identity,
)
from molt.cli.native_link_manifest import (
    NativeLinkDependencyManifestError,
    read_native_link_dependency_manifest,
    write_native_link_dependency_manifest,
)
from molt.cli.link_pipeline import _link_fingerprint
from molt.cli.runtime_fingerprints import (
    _runtime_artifact_fingerprint_matches,
    _write_runtime_fingerprint,
)
from molt.cli.runtime_build import _runtime_archives_semantically_match
from tools.native_link_benchmark import collect_input_facts


_SOURCE_FINGERPRINT = {
    "hash": "1" * 64,
    "inputs_digest": "2" * 64,
    "meta_digest": "3" * 64,
    "rustc": "rustc test",
}


def _header(name: str, size: int, *, timestamp: int) -> bytes:
    fields = (
        name.encode("ascii").ljust(16),
        str(timestamp).encode("ascii").ljust(12),
        b"0".ljust(6),
        b"0".ljust(6),
        b"100644".ljust(8),
        str(size).encode("ascii").ljust(10),
        b"`\n",
    )
    result = b"".join(fields)
    assert len(result) == 60
    return result


def _member(name: str, payload: bytes, *, timestamp: int) -> bytes:
    body = _header(name, len(payload), timestamp=timestamp) + payload
    return body + (b"\n" if len(payload) & 1 else b"")


def _archive(
    path: Path,
    *,
    style: str,
    rustc_id: str,
    timestamp: int = 0,
    symbol_table: bytes = b"derived-symbol-index",
    member_prefix: str = "",
    payloads: tuple[tuple[str, bytes], ...] = (
        ("alpha", b"object-alpha"),
        ("beta", b"object-beta"),
        ("dependency", b"object-dependency"),
    ),
) -> None:
    names = tuple(
        (
            (
                f"{member_prefix}molt_runtime.{token}.{rustc_id}.rcgu.o"
                if style == "coff"
                else (
                    f"{member_prefix}libmolt_runtime.molt_runtime.{rustc_id}"
                    f"-cgu.{index}.rcgu.o"
                )
            )
            if token != "dependency"
            else "dependency.0123456789abcdef-cgu.00.rcgu.o"
        )
        for index, (token, _payload) in enumerate(payloads)
    )
    parts = [b"!<arch>\n"]
    if style in {"gnu", "coff"}:
        parts.append(_member("/", symbol_table, timestamp=timestamp))
        separator = b"/\n" if style == "gnu" else b"\0"
        offsets: list[int] = []
        long_names = bytearray()
        for name in names:
            offsets.append(len(long_names))
            long_names.extend(name.encode("utf-8") + separator)
        parts.append(_member("//", bytes(long_names), timestamp=timestamp))
        for offset, (_token, payload) in zip(offsets, payloads, strict=True):
            parts.append(_member(f"/{offset}", payload, timestamp=timestamp))
    elif style == "bsd":
        parts.append(_member("__.SYMDEF/", symbol_table, timestamp=timestamp))
        for name, (_token, payload) in zip(names, payloads, strict=True):
            encoded_name = name.encode("utf-8")
            parts.append(
                _member(
                    f"#1/{len(encoded_name)}",
                    encoded_name + payload,
                    timestamp=timestamp,
                )
            )
    else:  # pragma: no cover - test helper contract
        raise AssertionError(style)
    path.write_bytes(b"".join(parts))


@pytest.mark.parametrize("style", ("coff", "gnu", "bsd"))
def test_semantic_identity_ignores_derived_tables_and_root_rustc_id(
    tmp_path: Path, style: str
) -> None:
    first = tmp_path / f"first-{style}.a"
    second = tmp_path / f"second-{style}.a"
    _archive(first, style=style, rustc_id="1ho179r", timestamp=0)
    _archive(
        second,
        style=style,
        rustc_id="19quida",
        timestamp=1_825_020_000,
        symbol_table=b"different-derived-symbol-index",
    )

    assert first.read_bytes() != second.read_bytes()
    assert static_archive_identity(first) == static_archive_identity(second)


@pytest.mark.parametrize("change", ("content", "member", "order"))
def test_semantic_identity_detects_link_relevant_change(
    tmp_path: Path, change: str
) -> None:
    baseline = tmp_path / "baseline.a"
    changed = tmp_path / "changed.a"
    payloads = (
        ("alpha", b"object-alpha"),
        ("beta", b"object-beta"),
        ("dependency", b"object-dependency"),
    )
    _archive(baseline, style="gnu", rustc_id="root123", payloads=payloads)
    if change == "content":
        changed_payloads = (payloads[0], ("beta", b"object-drift"), payloads[2])
    elif change == "member":
        changed_payloads = (payloads[0], payloads[1], ("renamed", payloads[2][1]))
    else:
        changed_payloads = (payloads[1], payloads[0], payloads[2])
    _archive(changed, style="gnu", rustc_id="root123", payloads=changed_payloads)

    assert static_archive_identity(baseline) != static_archive_identity(changed)


def test_byte_artifacts_keep_exact_identity_and_thin_archives_fail_closed(
    tmp_path: Path,
) -> None:
    artifact = tmp_path / "runtime.dll"
    artifact.write_bytes(b"first")
    before = artifact_content_identity(artifact)
    artifact.write_bytes(b"other")
    assert artifact_content_identity(artifact) != before

    thin = tmp_path / "runtime.a"
    thin.write_bytes(b"!<thin>\n")
    with pytest.raises(StaticArchiveIdentityError, match="not self-contained"):
        static_archive_identity(thin)

    malformed = tmp_path / "malformed.lib"
    malformed.write_bytes(b"!<arch>\nnot a complete header")
    with pytest.raises(StaticArchiveIdentityError, match="header is invalid"):
        artifact_content_identity(malformed)


def test_semantic_identity_does_not_normalize_unrelated_or_path_names(
    tmp_path: Path,
) -> None:
    baseline = tmp_path / "baseline.a"
    changed_dependency = tmp_path / "changed-dependency.a"
    path_changed = tmp_path / "path-changed.a"
    _archive(baseline, style="gnu", rustc_id="root123")
    _archive(
        changed_dependency,
        style="gnu",
        rustc_id="root123",
        payloads=(
            ("alpha", b"object-alpha"),
            ("beta", b"object-beta"),
            ("dependency-other", b"object-dependency"),
        ),
    )
    _archive(
        path_changed,
        style="gnu",
        rustc_id="root123",
        member_prefix="path/",
        payloads=(
            ("alpha", b"object-alpha"),
            ("beta", b"object-beta"),
            ("dependency", b"object-dependency"),
        ),
    )

    assert static_archive_identity(changed_dependency) != static_archive_identity(
        baseline
    )
    assert static_archive_identity(path_changed) != static_archive_identity(baseline)


def test_fingerprint_and_manifest_share_semantic_archive_authority(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir()
    fingerprint_path = tmp_path / "runtime.fingerprint.json"
    _archive(runtime, style="coff", rustc_id="first123")
    link_command = ["clang", str(runtime), "-o", "program"]
    initial_link = _link_fingerprint(
        project_root=tmp_path,
        inputs=[runtime],
        link_cmd=link_command,
    )
    assert initial_link is not None
    initial_benchmark = collect_input_facts({"runtime": runtime})
    _write_runtime_fingerprint(
        fingerprint_path, dict(_SOURCE_FINGERPRINT), artifact=runtime
    )
    write_native_link_dependency_manifest(
        "",
        cargo_stderr="note: native-static-libs: -lc\n",
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        source_root=tmp_path,
        source_fingerprint=_SOURCE_FINGERPRINT,
    )

    _archive(
        runtime,
        style="coff",
        rustc_id="second99",
        timestamp=42,
        symbol_table=b"regenerated-index",
    )
    scratch = tmp_path / "scratch" / "libmolt_runtime.a"
    scratch.parent.mkdir()
    _archive(scratch, style="coff", rustc_id="third777", timestamp=99)
    assert _runtime_archives_semantically_match(runtime, scratch)
    equivalent_link = _link_fingerprint(
        project_root=tmp_path,
        inputs=[runtime],
        link_cmd=link_command,
        stored_fingerprint=initial_link,
    )
    assert equivalent_link is not None
    assert equivalent_link["hash"] == initial_link["hash"]
    equivalent_benchmark = collect_input_facts({"runtime": runtime})
    assert equivalent_benchmark["fingerprint"] == initial_benchmark["fingerprint"]
    assert _runtime_artifact_fingerprint_matches(
        runtime,
        dict(_SOURCE_FINGERPRINT),
        fingerprint_path,
        require_artifact_digest=True,
    )
    read_native_link_dependency_manifest(
        runtime,
        target_triple=None,
        source_root=tmp_path,
        source_fingerprint=_SOURCE_FINGERPRINT,
    )

    _archive(
        runtime,
        style="coff",
        rustc_id="third456",
        payloads=(
            ("alpha", b"changed-code"),
            ("beta", b"object-beta"),
            ("dependency", b"object-dependency"),
        ),
    )
    assert not _runtime_archives_semantically_match(runtime, scratch)
    changed_link = _link_fingerprint(
        project_root=tmp_path,
        inputs=[runtime],
        link_cmd=link_command,
        stored_fingerprint=equivalent_link,
    )
    assert changed_link is not None
    assert changed_link["hash"] != equivalent_link["hash"]
    changed_benchmark = collect_input_facts({"runtime": runtime})
    assert changed_benchmark["fingerprint"] != equivalent_benchmark["fingerprint"]
    assert not _runtime_artifact_fingerprint_matches(
        runtime,
        dict(_SOURCE_FINGERPRINT),
        fingerprint_path,
        require_artifact_digest=True,
    )
    with pytest.raises(NativeLinkDependencyManifestError, match="digest mismatch"):
        read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            source_root=tmp_path,
            source_fingerprint=_SOURCE_FINGERPRINT,
        )
