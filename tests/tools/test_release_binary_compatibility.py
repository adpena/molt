"""The wheel-compatibility authority derives tags from real binaries only.

Linux/macOS cases run the pinned audit tools on genuine host binaries; they
skip where the host cannot supply the needed binary shape. Nothing here
fabricates tool output or compatibility receipts.
"""

from __future__ import annotations

import base64
import csv
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import sys
import zipfile

import pytest

from molt.verified_subset import current_host_coordinate
from molt.wheel_record import wheel_record_bytes
from tools.release import binary_compatibility as compat
from tools.release import release_model

PLATFORM, ARCH = current_host_coordinate()
_LINUX = PLATFORM == "linux" and importlib.util.find_spec("auditwheel") is not None
_MACOS = PLATFORM == "macos" and importlib.util.find_spec("delocate") is not None


@pytest.mark.skipif(
    not _LINUX, reason="release capability uses pinned Linux auditwheel"
)
@pytest.mark.parametrize("arch", ["x86_64", "aarch64"])
def test_linux_shared_library_capability_binds_maintained_policy(arch):
    from importlib.metadata import version
    from importlib.resources import files

    from auditwheel.architecture import Architecture
    from auditwheel.libc import Libc
    from auditwheel.policy import WheelPolicies

    raw = files("auditwheel.policy").joinpath("manylinux-policy.json").read_bytes()
    # Independently select the lowest-priority portable row from upstream data;
    # selecting the unconstrained linux row would silently permit any library.
    row = min(
        (
            row
            for row in json.loads(raw)
            if row["name"] != "linux" and arch in row["symbol_versions"]
        ),
        key=lambda row: row["priority"],
    )
    identity, libraries = compat.linux_shared_library_capability(arch)
    assert libraries == frozenset(row["lib_whitelist"])
    assert (
        libraries
        == WheelPolicies(libc=Libc.GLIBC, arch=Architecture(arch)).lowest.whitelist
    )
    assert isinstance(libraries, frozenset)
    assert dict(identity) == {
        "tool": "auditwheel",
        "version": version("auditwheel"),
        "platform": "linux",
        "arch": arch,
        "policy_name": f"{row['name']}_{arch}",
        "policy_sha256": hashlib.sha256(raw).hexdigest(),
        "shared_libraries": tuple(sorted(row["lib_whitelist"])),
    }
    with pytest.raises(TypeError):
        identity["policy_name"] = "linux"


@pytest.mark.skipif(
    not _LINUX, reason="release capability uses pinned Linux auditwheel"
)
def test_linux_shared_library_capability_rejects_unknown_architecture():
    with pytest.raises(ValueError):
        compat.linux_shared_library_capability("not-an-architecture")


@pytest.mark.parametrize(
    ("platform", "arch", "tag", "expected"),
    [
        ("windows", "x86_64", "win_amd64", True),
        ("windows", "arm64", "win_amd64", False),
        ("linux", "aarch64", "manylinux_2_28_aarch64", True),
        ("linux", "x86_64", "linux_x86_64", False),
        ("linux", "x86_64", "musllinux_1_2_x86_64", False),
        ("macos", "arm64", "macosx_14_0_arm64", True),
        ("macos", "arm64", "macosx_14_0_universal2", False),
    ],
)
def test_tag_shape_authority_is_shared(platform, arch, tag, expected):
    assert compat.wheel_platform_tag_matches(platform, arch, tag) is expected
    # Index admission uses the same authority as production and verification.
    assert release_model.wheel_platform_tag_matches is compat.wheel_platform_tag_matches


def test_tag_floor_orders_versions_numerically():
    assert compat.tag_floor(
        "linux", "x86_64", "manylinux_2_9_x86_64"
    ) < compat.tag_floor("linux", "x86_64", "manylinux_2_28_x86_64")
    with pytest.raises(ValueError):
        compat.tag_floor("windows", "x86_64", "win_amd64")


def test_windows_tags_are_exact_release_targets(tmp_path):
    for arch, tag in (("x86_64", "win_amd64"), ("arm64", "win_arm64")):
        derived = compat.derive_bundle_wheel_compatibility(
            tmp_path, platform="windows", arch=arch
        )
        assert derived.tag == tag


def test_bundle_without_native_members_has_no_binary_evidence(tmp_path):
    (tmp_path / "bin").mkdir()
    (tmp_path / "bin" / "molt-backend").write_bytes(b"#!/bin/sh\n")
    with pytest.raises(compat.WheelCompatibilityError, match="no native binaries"):
        compat.derive_bundle_wheel_compatibility(
            tmp_path, platform="linux", arch="x86_64"
        )


def test_audit_wheel_records_every_member_without_rereading(tmp_path, monkeypatch):
    member = tmp_path / "native"
    member.write_bytes(b"\x7fELF payload")
    original = Path.read_bytes
    reads = []

    def read_once(path):
        if path == member:
            reads.append(path)
        return original(path)

    monkeypatch.setattr(Path, "read_bytes", read_once)
    wheel = compat._write_audit_wheel(
        {'bin/comma,quote"': member}, tmp_path, "linux_x86_64"
    )
    assert reads == [member]
    with zipfile.ZipFile(wheel) as archive:
        names = archive.namelist()
        record_path = "molt_binary_audit-0.dist-info/RECORD"
        rows = list(csv.reader(io.StringIO(archive.read(record_path).decode("utf-8"))))
        assert len(rows) == len(names) == len({row[0] for row in rows})
        assert {row[0] for row in rows} == set(names)
        assert rows[-1] == [record_path, "", ""]
        assert "molt_binary_audit-0.dist-info/METADATA" in names
        assert "molt_binary_audit-0.dist-info/WHEEL" in names
        for name, digest, size in rows[:-1]:
            payload = archive.read(name)
            expected = base64.urlsafe_b64encode(hashlib.sha256(payload).digest())
            assert digest == "sha256=" + expected.rstrip(b"=").decode("ascii")
            assert int(size) == len(payload)


@pytest.mark.parametrize("duplicate", ["payload", "dist-info/RECORD"])
def test_record_serializer_rejects_duplicate_or_self_member(duplicate):
    rows = [("payload", hashlib.sha256(b"a").digest(), 1)]
    rows.append((duplicate, hashlib.sha256(b"b").digest(), 1))
    with pytest.raises(ValueError, match="duplicate wheel RECORD member"):
        wheel_record_bytes(rows, record_path="dist-info/RECORD")


def test_record_serializer_preserves_ordinary_member_bytes():
    # Published SHA-256 of the empty byte string; normal paths retain the old
    # LF-terminated wheel bytes while paths needing CSV escaping are quoted.
    assert wheel_record_bytes(
        [("empty.txt", hashlib.sha256(b"").digest(), 0)],
        record_path="pkg.dist-info/RECORD",
    ) == (
        b"empty.txt,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0\n"
        b"pkg.dist-info/RECORD,,\n"
    )


def _elf_needing(fragment: str) -> Path | None:
    """A real host executable whose dynamic section needs a non-policy library."""
    from auditwheel.lddtree import ldd

    for name in ("openssl", "curl", "wget", "ssh", "git"):
        found = shutil.which(name)
        if found is None:
            continue
        path = Path(found).resolve()
        try:
            if any(fragment in needed for needed in ldd(path).needed):
                return path
        except Exception:
            continue
    return None


def _tree_with(tmp_path: Path, executable: Path) -> Path:
    tree = tmp_path / "bundle"
    (tree / "bin").mkdir(parents=True)
    shutil.copy2(executable, tree / "bin" / "molt-backend")
    return tree


@pytest.mark.skipif(not _LINUX, reason="auditwheel audits Linux ELF binaries")
def test_linux_tag_is_derived_from_the_binaries_and_reaudits(tmp_path):
    # A relocated uv interpreter can lose its private libpython RUNPATH. This
    # control needs a genuine system-linked ELF, not Python installation custody.
    tree = _tree_with(tmp_path, Path("/usr/bin/true"))
    derived = compat.derive_bundle_wheel_compatibility(
        tree, platform="linux", arch=ARCH
    )
    assert derived.tag.startswith("manylinux_") and derived.tag.endswith(ARCH)
    assert derived.evidence["tool"] == "auditwheel"
    assert derived.evidence["versioned_symbols"]
    wheel = tmp_path / f"probe-0-py3-none-{derived.tag}.whl"
    with zipfile.ZipFile(wheel, "w") as archive:
        with zipfile.ZipFile(
            compat._write_audit_wheel(
                {'bin/comma,quote"': tree / "bin" / "molt-backend"},
                tmp_path,
                derived.tag,
            )
        ) as source:
            for info in source.infolist():
                archive.writestr(info, source.read(info))
    assert compat.audit_wheel(wheel, platform="linux", arch=ARCH).tag == derived.tag


@pytest.mark.skipif(not _LINUX, reason="auditwheel audits Linux ELF binaries")
def test_linux_external_library_closure_fails_with_evidence(tmp_path):
    executable = _elf_needing("libssl") or _elf_needing("libcurl")
    if executable is None:
        pytest.skip("no host executable needs a library outside manylinux policies")
    with pytest.raises(compat.WheelCompatibilityError) as error:
        compat.derive_bundle_wheel_compatibility(
            _tree_with(tmp_path, executable), platform="linux", arch=ARCH
        )
    assert error.value.evidence["external_libraries"]
    assert error.value.evidence["overall_policy"].startswith("linux_")


@pytest.mark.skipif(not _LINUX, reason="auditwheel audits Linux ELF binaries")
def test_linked_program_newer_than_the_claimed_floor_is_rejected(tmp_path):
    executable = Path("/usr/bin/true")
    derived = compat.audit_linked_executable(
        executable,
        platform="linux",
        arch=ARCH,
        claimed_tag=f"manylinux_99_0_{ARCH}",
    )
    older = f"manylinux_2_0_{ARCH}"
    if compat.tag_floor("linux", ARCH, derived.tag) <= (2, 0):
        pytest.skip("host executable has no glibc floor above 2.0")
    with pytest.raises(compat.WheelCompatibilityError, match="newer than"):
        compat.audit_linked_executable(
            executable, platform="linux", arch=ARCH, claimed_tag=older
        )


@pytest.mark.skipif(not _MACOS, reason="delocate audits Mach-O binaries")
def test_macos_tag_comes_from_load_commands(tmp_path):
    tree = _tree_with(tmp_path, Path("/usr/bin/true"))
    derived = compat.derive_bundle_wheel_compatibility(
        tree, platform="macos", arch=ARCH
    )
    assert derived.tag.startswith("macosx_") and derived.tag.endswith(ARCH)
    assert derived.evidence["minimum_versions"]["bin/molt-backend"]


@pytest.mark.skipif(not _MACOS, reason="delocate audits Mach-O binaries")
def test_macos_non_system_dylib_closure_fails_with_evidence(tmp_path):
    from delocate.delocating import filter_system_libs
    from delocate.libsana import get_dependencies

    executable = Path(sys.executable).resolve()
    if not any(
        path and filter_system_libs(path)
        for path, _name in get_dependencies(executable)
    ):
        pytest.skip("host interpreter links only system dylibs")
    with pytest.raises(compat.WheelCompatibilityError) as error:
        compat.derive_bundle_wheel_compatibility(
            _tree_with(tmp_path, executable), platform="macos", arch=ARCH
        )
    assert error.value.evidence["non_system_dependencies"] or error.value.evidence.get(
        "unresolved"
    )
