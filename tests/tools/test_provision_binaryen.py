from __future__ import annotations

from dataclasses import asdict, replace
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tarfile

import pytest

from molt import binaryen_identity
from molt.binaryen_identity import (
    BinaryenIdentityError,
    load_binaryen_install_receipt,
)
from molt.binaryen_toolchain import BinaryenHostAsset
from tools import provision_binaryen as provisioner
from tests.process_guard_common import install_module_view


_ARCHIVE_ROOT = "binaryen-version_130"
_ASSET_ID = "windows-x86_64" if os.name == "nt" else "linux-x86_64"
_EXECUTABLE = "bin/wasm-opt.exe" if os.name == "nt" else "bin/wasm-opt"


def _file(
    name: str,
    payload: bytes,
    *,
    mode: int = 0o644,
) -> tuple[tarfile.TarInfo, bytes]:
    info = tarfile.TarInfo(name)
    info.size = len(payload)
    info.mode = mode
    return info, payload


def _directory(name: str) -> tuple[tarfile.TarInfo, bytes]:
    info = tarfile.TarInfo(name)
    info.type = tarfile.DIRTYPE
    info.mode = 0o755
    return info, b""


def _symlink(name: str, target: str) -> tuple[tarfile.TarInfo, bytes]:
    info = tarfile.TarInfo(name)
    info.type = tarfile.SYMTYPE
    info.linkname = target
    return info, b""


def _fifo(name: str) -> tuple[tarfile.TarInfo, bytes]:
    info = tarfile.TarInfo(name)
    info.type = tarfile.FIFOTYPE
    return info, b""


def _required_members(
    *, executable_bytes: bytes = b"wasm-opt-v130"
) -> list[tuple[tarfile.TarInfo, bytes]]:
    return [
        _directory(f"{_ARCHIVE_ROOT}/"),
        _directory(f"{_ARCHIVE_ROOT}/bin/"),
        _file(
            f"{_ARCHIVE_ROOT}/{_EXECUTABLE}",
            executable_bytes,
            mode=0o755,
        ),
        _directory(f"{_ARCHIVE_ROOT}/include/"),
        _file(f"{_ARCHIVE_ROOT}/include/binaryen-c.h", b"header"),
    ]


def _write_archive(
    path: Path,
    members: list[tuple[tarfile.TarInfo, bytes]],
) -> None:
    with tarfile.open(path, "w:gz") as archive:
        for info, payload in members:
            archive.addfile(info, io.BytesIO(payload) if info.isfile() else None)


def _asset(archive: Path) -> BinaryenHostAsset:
    archive_bytes = archive.read_bytes()
    with tarfile.open(archive, "r:gz") as handle:
        identity = provisioner._archive_identity(
            handle,
            asset_id=_ASSET_ID,
            expected_root=_ARCHIVE_ROOT,
            executable=_EXECUTABLE,
        )
    record: dict[str, object] = {
        "id": _ASSET_ID,
        "version": "130",
        "url": "https://example.invalid/binaryen-version_130-x86_64-linux.tar.gz",
        "size": len(archive_bytes),
        "sha256": hashlib.sha256(archive_bytes).hexdigest(),
        "archive_root": _ARCHIVE_ROOT,
        "executable": _EXECUTABLE,
        "tree_entries": identity.tree.entries,
        "tree_total_bytes": identity.tree.total_bytes,
        "tree_sha256": identity.tree.sha256,
        "executable_sha256": identity.executable_sha256,
    }
    return BinaryenHostAsset(
        **record,
        record_sha256=hashlib.sha256(
            json.dumps(record, sort_keys=True, separators=(",", ":")).encode("utf-8")
        ).hexdigest(),
    )


def _install_asset(
    monkeypatch: pytest.MonkeyPatch,
    archive: Path,
    *,
    asset_archive: Path | None = None,
) -> tuple[BinaryenHostAsset, list[str]]:
    archive_bytes = archive.read_bytes()
    asset = _asset(asset_archive or archive)
    downloads: list[str] = []
    monkeypatch.setattr(provisioner, "binaryen_host_asset", lambda _root: asset)

    def download(
        url: str,
        output: Path,
        *,
        size: int,
        sha256: str,
    ) -> None:
        assert (url, size, sha256) == (asset.url, asset.size, asset.sha256)
        downloads.append(url)
        output.write_bytes(archive_bytes)

    monkeypatch.setattr(provisioner, "_download", download)
    monkeypatch.setattr(
        provisioner, "_read_wasm_opt_version", lambda _path, **_kwargs: "130"
    )
    return asset, downloads


def test_download_accepts_only_the_exact_manifest_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    payload = b"exact Binaryen archive bytes"
    monkeypatch.setattr(
        provisioner.urllib.request,
        "urlopen",
        lambda *_args, **_kwargs: io.BytesIO(payload),
    )
    output = tmp_path / "binaryen.tar.gz"

    provisioner._download(
        "https://example.invalid/binaryen.tar.gz",
        output,
        size=len(payload),
        sha256=hashlib.sha256(payload).hexdigest(),
    )

    assert output.read_bytes() == payload


@pytest.mark.parametrize("mismatch", ("size", "digest"))
def test_download_rejects_manifest_identity_mismatch(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mismatch: str,
) -> None:
    payload = b"substituted Binaryen archive bytes"
    monkeypatch.setattr(
        provisioner.urllib.request,
        "urlopen",
        lambda *_args, **_kwargs: io.BytesIO(payload),
    )

    with pytest.raises(ValueError, match="manifest identity"):
        provisioner._download(
            "https://example.invalid/binaryen.tar.gz",
            tmp_path / f"{mismatch}.tar.gz",
            size=len(payload) + (1 if mismatch == "size" else 0),
            sha256=(
                "0" * 64
                if mismatch == "digest"
                else hashlib.sha256(payload).hexdigest()
            ),
        )


@pytest.mark.parametrize(
    ("member", "message"),
    (
        (_file(f"{_ARCHIVE_ROOT}/../escape", b"escape"), "portable relative"),
        (_file("another-binaryen-root/escape", b"escape"), "outside"),
        (
            _file(f"{_ARCHIVE_ROOT}/{_EXECUTABLE.upper()}", b"collision"),
            "collision",
        ),
        (_symlink(f"{_ARCHIVE_ROOT}/bin/alias", "wasm-opt"), "unsupported node"),
        (_fifo(f"{_ARCHIVE_ROOT}/pipe"), "unsupported node"),
    ),
)
def test_archive_admission_rejects_traversal_wrong_root_collisions_and_links(
    tmp_path: Path,
    member: tuple[tarfile.TarInfo, bytes],
    message: str,
) -> None:
    archive_path = tmp_path / "invalid.tar.gz"
    _write_archive(archive_path, [*_required_members(), member])

    with tarfile.open(archive_path, "r:gz") as archive:
        with pytest.raises(ValueError, match=message):
            provisioner._archive_identity(
                archive,
                asset_id=_ASSET_ID,
                expected_root=_ARCHIVE_ROOT,
                executable=_EXECUTABLE,
            )


@pytest.mark.parametrize(
    ("field", "message"),
    (
        ("tree_sha256", "extracted tree differs"),
        ("executable_sha256", "executable differs"),
    ),
)
def test_installation_rejects_well_formed_manifest_identity_substitution(
    tmp_path: Path,
    field: str,
    message: str,
) -> None:
    archive_path = tmp_path / "binaryen.tar.gz"
    _write_archive(archive_path, _required_members())
    asset = _asset(archive_path)
    substituted = replace(asset, **{field: "0" * 64})

    extracted = tmp_path / "extracted"
    extracted.mkdir()
    provisioner._extract_archive_once(archive_path, extracted, asset)
    with pytest.raises(ValueError, match=message):
        provisioner._installation_identity(
            extracted / _ARCHIVE_ROOT,
            substituted,
        )


@pytest.mark.skipif(os.name == "nt", reason="POSIX archive modes are canonical")
def test_archive_admission_binds_posix_permission_modes(tmp_path: Path) -> None:
    archive_path = tmp_path / "binaryen.tar.gz"
    _write_archive(archive_path, _required_members())
    asset = _asset(archive_path)
    tampered_path = tmp_path / "permission-tampered.tar.gz"
    tampered = _required_members()
    executable = next(
        member
        for member in tampered
        if member[0].name == f"{_ARCHIVE_ROOT}/{_EXECUTABLE}"
    )
    executable[0].mode = 0o644
    _write_archive(tampered_path, tampered)

    extracted = tmp_path / "extracted"
    extracted.mkdir()
    provisioner._extract_archive_once(tampered_path, extracted, asset)
    with pytest.raises(ValueError, match="extracted tree differs"):
        provisioner._installation_identity(extracted / _ARCHIVE_ROOT, asset)


def test_windows_archive_identity_normalizes_permission_modes(tmp_path: Path) -> None:
    canonical_path = tmp_path / "canonical.tar.gz"
    _write_archive(canonical_path, _required_members())
    tampered = _required_members()
    for info, _payload in tampered:
        info.mode = 0o700 if info.isdir() else 0o600
    tampered_path = tmp_path / "mode-varied.tar.gz"
    _write_archive(tampered_path, tampered)

    identities = []
    for archive_path in (canonical_path, tampered_path):
        with tarfile.open(archive_path, "r:gz") as archive:
            identities.append(
                provisioner._archive_identity(
                    archive,
                    asset_id="windows-x86_64",
                    expected_root=_ARCHIVE_ROOT,
                    executable=_EXECUTABLE,
                ).tree
            )

    assert identities[0] == identities[1]


@pytest.mark.skipif(os.name == "nt", reason="POSIX umask and directory modes")
@pytest.mark.parametrize("umask", [0o077, 0o027, 0o002])
def test_provision_preserves_admitted_directory_modes_independent_of_umask(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, umask: int
) -> None:
    archive = tmp_path / "binaryen.tar.gz"
    members = _required_members()
    for name in ("include/readonly", "include/readonly/nested"):
        directory = _directory(f"{_ARCHIVE_ROOT}/{name}")
        directory[0].mode = 0o555
        members.append(directory)
    members.append(_file(f"{_ARCHIVE_ROOT}/include/readonly/nested/value", b"complete"))
    _write_archive(archive, members)
    asset, downloads = _install_asset(monkeypatch, archive)
    output = tmp_path / "installed-binaryen"
    previous = os.umask(umask)
    try:
        assert provisioner.provision_binaryen(output) == output
        for name in (".", "bin", "include"):
            assert stat.S_IMODE((output / name).stat().st_mode) == 0o755
        for name in ("include/readonly", "include/readonly/nested"):
            assert stat.S_IMODE((output / name).stat().st_mode) == 0o555
        assert (output / "include/readonly/nested/value").read_bytes() == b"complete"
        assert provisioner.provision_binaryen(output) == output
        assert downloads == [asset.url]
        assert load_binaryen_install_receipt(output)["asset"] == asdict(asset)
    finally:
        os.umask(previous)
        # Restore only this fixture's read-only directories for portable cleanup.
        for name in ("include/readonly", "include/readonly/nested"):
            directory = output / name
            if directory.is_dir():
                directory.chmod(0o755)


@pytest.mark.parametrize("mode", [0o1777, 0o2755, 0o775])
def test_posix_extraction_does_not_restore_unsafe_directory_modes(
    tmp_path: Path, mode: int
) -> None:
    archive = tmp_path / "binaryen.tar.gz"
    members = _required_members()
    members[0][0].mode = mode
    _write_archive(archive, members)
    asset = replace(_asset(archive), id="linux-x86_64")
    extracted = tmp_path / "extracted"
    extracted.mkdir()
    with pytest.raises(ValueError, match="directory has an unsafe mode"):
        provisioner._extract_archive_once(archive, extracted, asset)
    assert not (extracted / _ARCHIVE_ROOT).exists()


def test_posix_extraction_restores_directory_modes_after_children(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive = tmp_path / "binaryen.tar.gz"
    members = _required_members()
    parent = _directory(f"{_ARCHIVE_ROOT}/include/readonly")
    parent[0].mode = 0o555
    members.extend(
        [parent, _file(f"{_ARCHIVE_ROOT}/include/readonly/value", b"complete")]
    )
    _write_archive(archive, members)
    asset = replace(_asset(archive), id="linux-x86_64")
    extracted = tmp_path / "extracted"
    extracted.mkdir()
    installation = extracted / _ARCHIVE_ROOT
    chmod = Path.chmod
    calls = []

    def record_directory_mode(path: Path, mode: int, **kwargs) -> None:
        if path.is_dir():
            assert (installation / "include/readonly/value").read_bytes() == b"complete"
            calls.append((path.relative_to(installation).as_posix(), mode))
        else:
            chmod(path, mode, **kwargs)

    # Exercise POSIX asset policy on every host; actual umask/mode semantics
    # remain the separate POSIX-only public provisioning discriminator above.
    monkeypatch.setattr(Path, "chmod", record_directory_mode)
    provisioner._extract_archive_once(archive, extracted, asset)
    assert dict(calls) == {
        ".": 0o755,
        "bin": 0o755,
        "include": 0o755,
        "include/readonly": 0o555,
    }
    assert len(calls) == 4
    order = [name for name, _mode in calls]
    assert order.index("include/readonly") < order.index("include") < order.index(".")
    assert order.index("bin") < order.index(".")


def test_provision_publishes_only_a_complete_verified_installation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "binaryen.tar.gz"
    _write_archive(archive_path, _required_members())
    asset, downloads = _install_asset(monkeypatch, archive_path)
    output = tmp_path / "installed-binaryen"

    installed = provisioner.provision_binaryen(output)

    assert installed == output.resolve()
    assert installed.joinpath(*_EXECUTABLE.split("/")).read_bytes() == (
        b"wasm-opt-v130"
    )
    receipt = load_binaryen_install_receipt(installed)
    assert receipt["schema"] == "molt.binaryen-install.v3"
    assert receipt["asset"] == asdict(asset)
    assert receipt["tree"]["sha256"] == asset.tree_sha256
    assert downloads == [asset.url]
    assert list(tmp_path.glob("molt-binaryen-*")) == []


def test_provision_hot_path_does_not_rehash_the_archive_tree(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "binaryen.tar.gz"
    _write_archive(archive_path, _required_members())
    _install_asset(monkeypatch, archive_path)
    monkeypatch.setattr(
        provisioner,
        "_archive_identity",
        lambda *_args, **_kwargs: pytest.fail(
            "production provisioning recomputed the offline archive identity"
        ),
    )

    provisioner.provision_binaryen(tmp_path / "installed-binaryen")


def test_provision_reuses_only_an_exact_receipted_tree_without_redownload(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "binaryen.tar.gz"
    _write_archive(archive_path, _required_members())
    asset, downloads = _install_asset(monkeypatch, archive_path)
    output = tmp_path / "installed-binaryen"
    first = provisioner.provision_binaryen(output)

    second = provisioner.provision_binaryen(output)

    assert first == second == output.resolve()
    assert downloads == [asset.url]


def test_reuse_fails_closed_after_tree_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "binaryen.tar.gz"
    _write_archive(archive_path, _required_members())
    _install_asset(monkeypatch, archive_path)
    output = provisioner.provision_binaryen(tmp_path / "installed-binaryen")
    output.joinpath(*_EXECUTABLE.split("/")).write_bytes(b"mutated")

    with pytest.raises(ValueError, match="extracted tree differs"):
        provisioner.provision_binaryen(output)


@pytest.mark.skipif(sys.platform == "win32", reason="POSIX execute bits only")
def test_reuse_fails_closed_after_executable_permission_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "binaryen.tar.gz"
    _write_archive(archive_path, _required_members())
    _install_asset(monkeypatch, archive_path)
    output = provisioner.provision_binaryen(tmp_path / "installed-binaryen")
    wasm_opt = output.joinpath(*_EXECUTABLE.split("/"))
    wasm_opt.chmod(0o644)

    with pytest.raises(ValueError, match="extracted tree differs"):
        provisioner.provision_binaryen(output)


@pytest.mark.skipif(sys.platform == "win32", reason="POSIX root mode only")
def test_reuse_fails_closed_after_root_permission_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "binaryen.tar.gz"
    _write_archive(archive_path, _required_members())
    _install_asset(monkeypatch, archive_path)
    output = provisioner.provision_binaryen(tmp_path / "installed-binaryen")
    output.chmod(0o700)

    with pytest.raises(ValueError, match="extracted tree differs"):
        provisioner.provision_binaryen(output)


def test_reuse_fails_closed_after_receipt_asset_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "binaryen.tar.gz"
    _write_archive(archive_path, _required_members())
    _install_asset(monkeypatch, archive_path)
    output = provisioner.provision_binaryen(tmp_path / "installed-binaryen")
    receipt_path = output / ".molt-binaryen-source.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    receipt["asset"]["version"] = "131"
    receipt_path.write_text(json.dumps(receipt), encoding="utf-8")

    with pytest.raises(ValueError, match="existing Binaryen installation is invalid"):
        provisioner.provision_binaryen(output)


@pytest.mark.parametrize(
    ("field", "value"),
    (
        ("entries", 1),
        ("total_bytes", 1),
        ("sha256", "0" * 64),
    ),
)
def test_receipt_rejects_well_formed_tree_identity_substitution(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    field: str,
    value: int | str,
) -> None:
    archive_path = tmp_path / "binaryen.tar.gz"
    _write_archive(archive_path, _required_members())
    _install_asset(monkeypatch, archive_path)
    output = provisioner.provision_binaryen(tmp_path / "installed-binaryen")
    receipt_path = output / ".molt-binaryen-source.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    receipt["tree"][field] = value
    receipt_path.write_text(json.dumps(receipt), encoding="utf-8")

    with pytest.raises(ValueError, match="provision receipt identity is invalid"):
        provisioner.provision_binaryen(output)


def test_provision_failure_never_publishes_a_partial_installation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "incomplete.tar.gz"
    members = [
        member
        for member in _required_members()
        if member[0].name != f"{_ARCHIVE_ROOT}/{_EXECUTABLE}"
    ]
    _write_archive(archive_path, members)
    valid_archive = tmp_path / "valid.tar.gz"
    _write_archive(valid_archive, _required_members())
    _install_asset(monkeypatch, archive_path, asset_archive=valid_archive)
    output = tmp_path / "installed-binaryen"

    with pytest.raises(ValueError, match="missing required executable"):
        provisioner.provision_binaryen(output)

    assert not output.exists()
    assert list(tmp_path.glob("molt-binaryen-*")) == []


def test_provision_rejects_executable_version_mismatch_before_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "binaryen.tar.gz"
    _write_archive(archive_path, _required_members())
    _install_asset(monkeypatch, archive_path)
    monkeypatch.setattr(
        provisioner, "_read_wasm_opt_version", lambda _path, **_kwargs: "131"
    )
    output = tmp_path / "installed-binaryen"

    with pytest.raises(ValueError, match="executable identity differs"):
        provisioner.provision_binaryen(output)

    assert not output.exists()
    assert list(tmp_path.glob("molt-binaryen-*")) == []


def test_provision_rejects_a_dangling_output_symlink(tmp_path: Path) -> None:
    output = tmp_path / "installed-binaryen"
    try:
        output.symlink_to(tmp_path / "missing-binaryen", target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"host cannot create a test symlink: {exc}")

    with pytest.raises(ValueError, match="existing Binaryen installation is invalid"):
        provisioner.provision_binaryen(output)


@pytest.mark.parametrize("reuse", [False, True])
@pytest.mark.parametrize("mutation", ["before", "during-same", "during-different"])
def test_version_probe_rejects_executable_replacement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, reuse: bool, mutation: str
) -> None:
    archive = tmp_path / "binaryen.tar.gz"
    _write_archive(archive, _required_members(executable_bytes=b"\x7fELFfixture"))
    read_version = provisioner._read_wasm_opt_version
    _install_asset(monkeypatch, archive)
    output = tmp_path / "installed-binaryen"
    if reuse:
        provisioner.provision_binaryen(output)
    monkeypatch.setattr(provisioner, "_read_wasm_opt_version", read_version)
    commands = []

    def replace_executable(path: Path, *, same_bytes: bool) -> None:
        replacement = path.with_name(path.name + ".replacement")
        replacement.write_bytes(path.read_bytes() if same_bytes else b"\x7fELFchanged")
        replacement.chmod(0o755)
        replacement.replace(path)

    if mutation == "before":
        identify = provisioner._installation_identity

        def identify_then_replace(root, asset):
            observed = identify(root, asset)
            replace_executable(root / _EXECUTABLE, same_bytes=False)
            return observed

        monkeypatch.setattr(
            provisioner, "_installation_identity", identify_then_replace
        )

    def completed(command, **kwargs):
        assert command[1:] == ["--version"]
        commands.append(list(command))
        if mutation != "before":
            replace_executable(Path(command[0]), same_bytes=mutation == "during-same")
        return subprocess.CompletedProcess(
            command, 0, "wasm-opt version 130 (version_130)\n", ""
        )

    # Both source versions are safe to discriminate offline: the preserved
    # donor's raw call and the reconciled shared boundary are mocked here.
    install_module_view(
        monkeypatch, "subprocess", subprocess, binaryen_identity, run=completed
    )
    monkeypatch.setattr(
        binaryen_identity, "run_completed_command", completed, raising=False
    )
    with pytest.raises(BinaryenIdentityError):
        provisioner.provision_binaryen(output)
    assert len(commands) == (0 if mutation == "before" else 1)
    assert output.exists() is reuse
    assert list(tmp_path.glob("molt-binaryen-*")) == []


@pytest.mark.parametrize("occupied", [False, True])
def test_publication_preserves_a_racing_destination(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, occupied: bool
) -> None:
    archive = tmp_path / "binaryen.tar.gz"
    _write_archive(archive, _required_members())
    _install_asset(monkeypatch, archive)
    output = tmp_path / "installed-binaryen"
    publish = provisioner.durable_publish_directory_exclusive

    def occupy_then_publish(staged: Path, destination: Path) -> None:
        assert destination == output
        destination.mkdir()
        if occupied:
            (destination / "owned.txt").write_bytes(b"other publisher")
        publish(staged, destination)

    monkeypatch.setattr(
        provisioner, "durable_publish_directory_exclusive", occupy_then_publish
    )
    with pytest.raises(FileExistsError):
        provisioner.provision_binaryen(output)
    assert sorted(path.name for path in output.iterdir()) == (
        ["owned.txt"] if occupied else []
    )
    if occupied:
        assert (output / "owned.txt").read_bytes() == b"other publisher"
    assert list(tmp_path.glob("molt-binaryen-*")) == []


@pytest.mark.parametrize("reuse", [False, True])
def test_provision_probes_the_manifest_admitted_executable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, reuse: bool
) -> None:
    archive = tmp_path / "binaryen.tar.gz"
    executable_bytes = b"\x7fELFfixture"
    _write_archive(archive, _required_members(executable_bytes=executable_bytes))
    read_version = provisioner._read_wasm_opt_version
    asset, downloads = _install_asset(monkeypatch, archive)
    output = tmp_path / "installed-binaryen"
    if reuse:
        provisioner.provision_binaryen(output)
    monkeypatch.setattr(provisioner, "_read_wasm_opt_version", read_version)
    probes = []

    def completed(command, **_kwargs):
        assert command[1:] == ["--version"]
        assert Path(command[0]).read_bytes() == executable_bytes
        probes.append(command)
        return subprocess.CompletedProcess(
            command, 0, "wasm-opt version 130 (version_130)\n", ""
        )

    monkeypatch.setattr(binaryen_identity, "run_completed_command", completed)
    assert provisioner.provision_binaryen(output) == output
    assert len(probes) == 1
    assert downloads == [asset.url]
    receipt = load_binaryen_install_receipt(output)
    assert receipt["asset"] == asdict(asset)
    assert (output / _EXECUTABLE).read_bytes() == executable_bytes
    assert list(tmp_path.glob("molt-binaryen-*")) == []


def _native_probe_fixture(tmp_path: Path) -> tuple[Path, str]:
    executable = tmp_path / Path(_EXECUTABLE).name
    executable.write_bytes(b"\x7fELFfixture")
    executable.chmod(0o755)
    return executable, hashlib.sha256(executable.read_bytes()).hexdigest()


@pytest.mark.parametrize(
    "stdout",
    (
        "wasm-opt version 130",
        "wasm-opt version 130 (version_131)",
        "wasm-opt version 130 (version_130) trailing",
        "wasm-opt version 130 (version_130)\nsecond line",
        "wasm-opt version 130 (version_130)wasm-opt version 130 (version_130)",
    ),
)
def test_version_probe_rejects_noncanonical_or_ambiguous_output(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    stdout: str,
) -> None:
    executable, digest = _native_probe_fixture(tmp_path)
    monkeypatch.setattr(
        binaryen_identity,
        "run_completed_command",
        lambda *_args, **_kwargs: subprocess.CompletedProcess([], 0, stdout, ""),
    )

    with pytest.raises(ValueError, match="invalid version"):
        provisioner._read_wasm_opt_version(executable, expected_sha256=digest)


def test_version_probe_accepts_only_the_canonical_upstream_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    executable, digest = _native_probe_fixture(tmp_path)
    calls = []

    def completed(command, **kwargs):
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(
            command, 0, "wasm-opt version 130 (version_130)\n", ""
        )

    monkeypatch.setattr(binaryen_identity, "run_completed_command", completed)
    assert (
        provisioner._read_wasm_opt_version(executable, expected_sha256=digest) == "130"
    )
    assert calls == [
        (
            [str(executable), "--version"],
            {
                "memory_guard_prefix": None,
                "check": False,
                "capture_output": True,
                "text": True,
                "encoding": "utf-8",
                "errors": "strict",
                "timeout": 30,
            },
        )
    ]


@pytest.mark.parametrize("failure", ["status", "stderr", "utf8", "timeout"])
def test_version_probe_rejects_process_and_encoding_failures(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failure: str
) -> None:
    executable, digest = _native_probe_fixture(tmp_path)

    def completed(command, **_kwargs):
        if failure == "utf8":
            raise UnicodeDecodeError("utf-8", b"\xff", 0, 1, "invalid byte")
        if failure == "timeout":
            raise subprocess.TimeoutExpired(command, 30)
        return subprocess.CompletedProcess(
            command,
            1 if failure == "status" else 0,
            "wasm-opt version 130 (version_130)\n",
            "unexpected stderr" if failure == "stderr" else "",
        )

    monkeypatch.setattr(binaryen_identity, "run_completed_command", completed)
    with pytest.raises(BinaryenIdentityError):
        provisioner._read_wasm_opt_version(executable, expected_sha256=digest)


def test_version_probe_preserves_windows_text_newline_contract(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    executable, digest = _native_probe_fixture(tmp_path)
    probes = []

    def completed(command, **kwargs):
        probes.append(command)
        assert kwargs["text"] is True
        assert kwargs["encoding"] == "utf-8"
        assert kwargs["errors"] == "strict"
        assert kwargs["timeout"] == 30
        # subprocess text mode uses this standard-library universal-newline
        # contract. Exercise the real shared runner, mocking only its OS call.
        with io.TextIOWrapper(
            io.BytesIO(b"wasm-opt version 130 (version_130)\r\n"),
            encoding=kwargs["encoding"],
            errors=kwargs["errors"],
        ) as stream:
            stdout = stream.read()
        return subprocess.CompletedProcess(command, 0, stdout, "")

    monkeypatch.setattr(subprocess, "run", completed)
    assert (
        provisioner._read_wasm_opt_version(executable, expected_sha256=digest) == "130"
    )
    assert probes == [[str(executable), "--version"]]


def test_cli_projects_exact_root_and_wasm_opt_outputs(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root = (tmp_path / "binaryen").resolve()
    asset = BinaryenHostAsset(
        id="windows-x86_64",
        version="130",
        url="https://example.invalid/binaryen.tar.gz",
        size=1,
        sha256="0" * 64,
        archive_root=_ARCHIVE_ROOT,
        executable="bin/wasm-opt.exe",
        tree_entries=1,
        tree_total_bytes=1,
        tree_sha256="2" * 64,
        executable_sha256="3" * 64,
        record_sha256="1" * 64,
    )
    github_output = tmp_path / "github-output"
    monkeypatch.setattr(provisioner, "provision_binaryen", lambda _output: root)
    monkeypatch.setattr(provisioner, "binaryen_host_asset", lambda _root: asset)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "provision_binaryen",
            "--output",
            str(root),
            "--github-output",
            str(github_output),
        ],
    )

    provisioner.main()

    assert capsys.readouterr().out == f"{root}\n"
    assert github_output.read_text(encoding="utf-8") == (
        f"root={root}\nwasm_opt={root / 'bin/wasm-opt.exe'}\n"
    )


@pytest.mark.parametrize(
    ("payload", "message"),
    (
        (
            '{"schema":"molt.binaryen-install.v3","schema":"duplicate"}\n',
            "duplicate JSON key",
        ),
        ('{"schema":NaN}\n', "non-finite JSON number"),
    ),
)
def test_receipt_loader_rejects_inexact_json(
    tmp_path: Path, payload: str, message: str
) -> None:
    root = tmp_path / "binaryen"
    root.mkdir()
    (root / ".molt-binaryen-source.json").write_text(
        payload,
        encoding="utf-8",
    )

    with pytest.raises(BinaryenIdentityError, match=message):
        load_binaryen_install_receipt(root)
