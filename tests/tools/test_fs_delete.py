"""Cross-platform teeth for the canonical artifact deletion primitive."""

from __future__ import annotations

import ctypes
import errno
import os
import stat
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt import file_deletion
from tools import disk_guard


def test_cleanup_authorities_share_one_deletion_primitive() -> None:
    assert disk_guard.delete_path is file_deletion.delete_path


def test_delete_path_removes_nested_readonly_artifacts(tmp_path: Path) -> None:
    tree = tmp_path / "cargo-fixture"
    nested = tree / ".git" / "objects" / "pack"
    nested.mkdir(parents=True)
    readonly = nested / "pack-test.pack"
    readonly.write_bytes(b"artifact")
    readonly.chmod(stat.S_IREAD)

    ok, error = file_deletion.delete_path(tree)

    assert ok, error
    assert not tree.exists()


def test_delete_path_removes_readonly_file(tmp_path: Path) -> None:
    readonly = tmp_path / "receipt.json"
    readonly.write_text("{}", encoding="utf-8")
    readonly.chmod(stat.S_IREAD)

    ok, error = file_deletion.delete_path(readonly)

    assert ok, error
    assert not readonly.exists()


@pytest.mark.parametrize("operation", [os.scandir, os.open, os.lstat])
def test_rmtree_callback_preserves_non_deletion_failures(
    tmp_path: Path, operation
) -> None:
    error = PermissionError("enumeration or access denied")
    with pytest.raises(OSError) as raised:
        file_deletion._rmtree_error(operation, str(tmp_path), error)
    assert raised.value is error


def test_rmtree_callback_preserves_non_permission_failures(tmp_path: Path) -> None:
    error = OSError("invalid filesystem operation")
    with pytest.raises(OSError) as raised:
        file_deletion._rmtree_error(os.unlink, str(tmp_path), error)
    assert raised.value is error


@pytest.mark.skipif(os.name == "nt", reason="POSIX permission failure contract")
def test_posix_permission_failure_does_not_change_attributes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "denied"
    path.write_bytes(b"preserve")
    original_mode = path.stat().st_mode
    error = PermissionError("parent does not permit deletion")

    def denied(*_args, **_kwargs):
        raise error

    monkeypatch.setattr(Path, "unlink", denied)
    with pytest.raises(PermissionError) as raised:
        file_deletion.unlink_file(path)
    assert raised.value is error
    assert file_deletion.delete_path(path) == (False, str(error))
    assert path.stat().st_mode == original_mode
    assert path.read_bytes() == b"preserve"


def test_deletion_accepts_absence_and_unlink_rejects_real_directories(tmp_path: Path):
    missing = tmp_path / "missing"
    assert file_deletion.delete_path(missing) == (True, "")
    file_deletion.unlink_file(missing)
    file_deletion._rmtree_error(os.unlink, str(missing), FileNotFoundError())
    directory = tmp_path / "directory"
    directory.mkdir()
    with pytest.raises(OSError) as rejected:
        file_deletion.unlink_file(directory)
    # Darwin reports EPERM for unlink(directory); Linux reports EISDIR.
    assert rejected.value.errno in (errno.EPERM, errno.EISDIR)
    assert directory.is_dir()


def test_unlink_rejects_embedded_nul_without_removing_prefix(tmp_path: Path) -> None:
    prefix = tmp_path / "preserve"
    prefix.write_bytes(b"prefix must survive")
    with pytest.raises(ValueError, match="null"):
        file_deletion.unlink_file(Path(str(prefix) + "\0suffix"))
    assert prefix.read_bytes() == b"prefix must survive"


def test_writable_unlink_does_not_load_windows_api(tmp_path, monkeypatch):
    leaf = tmp_path / "writable"
    leaf.write_bytes(b"remove")
    monkeypatch.setattr(
        file_deletion,
        "_windows_delete_api",
        lambda: pytest.fail("ordinary unlink must not load the native retry API"),
    )
    file_deletion.unlink_file(leaf)
    assert not leaf.exists()


@pytest.mark.parametrize("operation", ["unlink", "file", "tree"])
def test_readonly_hardlink_deletion_preserves_source_handle_attributes(
    tmp_path: Path, readonly_file_source, operation: str
) -> None:
    source, attributes = readonly_file_source
    before = attributes()
    identity = (source.stat().st_dev, source.stat().st_ino)
    owned = tmp_path / "owned"
    owned.mkdir()
    link = owned / "borrowed.exe"
    link.hardlink_to(source)

    def forbidden_chmod(*_args, **_kwargs):
        pytest.fail("deletion must not change attributes shared by another hardlink")

    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(Path, "chmod", forbidden_chmod)
        if operation == "unlink":
            file_deletion.unlink_file(link)
        else:
            assert file_deletion.delete_path(
                owned if operation == "tree" else link
            ) == (True, "")
    assert not link.exists()
    assert attributes() == before
    assert (source.stat().st_dev, source.stat().st_ino) == identity
    assert source.read_bytes() == b"external source must survive cleanup"


@pytest.mark.parametrize("directory", [False, True])
@pytest.mark.parametrize("nested", [False, True])
def test_deletion_preserves_symlink_referent(
    tmp_path: Path, directory: bool, nested: bool
) -> None:
    outside = tmp_path / "outside"
    if directory:
        outside.mkdir()
        payload = outside / "payload"
    else:
        payload = outside
    payload.write_bytes(b"preserve referent")
    payload.chmod(0o444)
    owned = tmp_path / "owned"
    owned.mkdir()
    link = owned / "link"
    # Failure to create the required Windows link is an unverified cell, not
    # a passing deletion control. The supported CI host must permit symlinks.
    link.symlink_to(outside, target_is_directory=directory)
    try:
        if os.name == "nt" and not nested:
            # Exercise the actual native retry owner even when DeleteFileW
            # could remove this link without entering the readonly branch.
            file_deletion._windows_delete_leaf(link, directory=False)
            assert not link.is_symlink()
            assert payload.read_bytes() == b"preserve referent"
            assert not payload.stat().st_mode & stat.S_IWRITE
            link.symlink_to(outside, target_is_directory=directory)
        assert file_deletion.delete_path(owned if nested else link) == (True, "")
        assert not link.is_symlink()
        assert payload.read_bytes() == b"preserve referent"
        assert not payload.stat().st_mode & stat.S_IWRITE
    finally:
        payload.chmod(0o600)


def test_delete_dangling_symlink(tmp_path: Path) -> None:
    link = tmp_path / "dangling"
    link.symlink_to(tmp_path / "absent")
    assert file_deletion.delete_path(link) == (True, "")
    assert not link.is_symlink()


@pytest.fixture
def windows_delete_calls(monkeypatch: pytest.MonkeyPatch):
    """Exercise handle ownership/errors on every host; real IO tests remain above."""

    class Metadata(ctypes.Structure):
        _fields_ = [
            ("FileAttributes", ctypes.c_uint32),
            ("ReparseTag", ctypes.c_uint32),
        ]

    state = SimpleNamespace(
        calls=[], fail_at=None, close_fails=False, attributes=0, error_code=0
    )

    def create(*args):
        state.calls.append(("open", args))
        if state.fail_at == "open":
            state.error_code = 5
            return ctypes.c_void_p(-1).value
        return 123

    def query(handle, kind, buffer, size):
        state.calls.append(("query", handle, kind, size))
        if state.fail_at == "query":
            state.error_code = 87
            return 0
        ctypes.cast(
            buffer, ctypes.POINTER(Metadata)
        ).contents.FileAttributes = state.attributes
        return 1

    def dispose(handle, kind, buffer, size):
        flags = ctypes.cast(buffer, ctypes.POINTER(ctypes.c_uint32)).contents.value
        state.calls.append(("dispose", handle, kind, flags, size))
        if state.fail_at == "dispose":
            state.error_code = 50
            return 0
        return 1

    def close(handle):
        state.calls.append(("close", handle))
        # Change the thread error even on success to catch deferred error reads.
        state.error_code = 6
        return not state.close_fails

    fake_ctypes = SimpleNamespace(
        c_void_p=ctypes.c_void_p,
        c_uint32=ctypes.c_uint32,
        byref=ctypes.byref,
        sizeof=ctypes.sizeof,
        get_last_error=lambda: state.error_code,
        WinError=lambda code: OSError(code, f"Windows error {code}"),
    )
    api = SimpleNamespace(
        CreateFileW=create,
        GetFileInformationByHandleEx=query,
        SetFileInformationByHandle=dispose,
        CloseHandle=close,
    )
    monkeypatch.setattr(
        file_deletion, "_windows_delete_api", lambda: (fake_ctypes, api, Metadata)
    )
    return state


def test_windows_delete_uses_nofollow_posix_disposition_and_closes(
    windows_delete_calls,
):
    state = windows_delete_calls
    file_deletion._windows_delete_leaf(Path("owned"), directory=False)
    assert state.calls == [
        ("open", ("owned", 0x10180, 0x7, None, 3, 0x02200000, None)),
        ("query", 123, 9, 8),
        ("dispose", 123, 21, 0x13, 4),
        ("close", 123),
    ]


@pytest.mark.parametrize(
    "stage,close_fails,code",
    [
        ("open", False, 5),
        ("query", False, 87),
        ("dispose", False, 50),
        (None, True, 6),
        ("query", True, 87),
        ("dispose", True, 50),
    ],
)
def test_windows_delete_preserves_primary_failure_without_fallback(
    windows_delete_calls, stage, code, close_fails
):
    state = windows_delete_calls
    state.fail_at = stage
    state.close_fails = close_fails
    try:
        raise PermissionError("original rmtree failure")
    except PermissionError:
        with pytest.raises(OSError) as raised:
            file_deletion._windows_delete_leaf(Path("owned"), directory=False)
    assert raised.value.errno == code
    assert raised.value.filename == "owned"
    assert [call[0] for call in state.calls].count("open") == 1
    assert [call[0] for call in state.calls].count("close") == (
        0 if stage == "open" else 1
    )
    if close_fails and stage in ("query", "dispose"):
        assert "closing deletion handle also failed" in raised.value.__notes__[0]


@pytest.mark.parametrize(
    "directory,attributes,error",
    [(False, 0x10, IsADirectoryError), (True, 0, NotADirectoryError)],
)
def test_windows_delete_validates_opened_type_before_disposition(
    windows_delete_calls, directory, attributes, error
):
    state = windows_delete_calls
    state.attributes = attributes
    with pytest.raises(error):
        file_deletion._windows_delete_leaf(Path("owned"), directory=directory)
    assert [call[0] for call in state.calls] == ["open", "query", "close"]


@pytest.mark.parametrize("directory", [False, True])
def test_windows_rmtree_callback_retries_only_the_failed_leaf(
    tmp_path, monkeypatch, directory
):
    operation = os.rmdir if directory else os.unlink
    calls = []
    monkeypatch.setattr(
        file_deletion,
        "os",
        SimpleNamespace(name="nt", unlink=os.unlink, rmdir=os.rmdir),
    )
    monkeypatch.setattr(
        file_deletion,
        "_windows_delete_leaf",
        lambda path, **kwargs: calls.append((path, kwargs["directory"])),
    )
    leaf = tmp_path / "owned"
    file_deletion._rmtree_error(operation, str(leaf), PermissionError("readonly"))
    assert calls == [(leaf, directory)]


@pytest.mark.skipif(os.name != "nt", reason="Windows sharing denial and recovery")
def test_windows_sharing_failure_preserves_readonly_source_then_recovers(
    tmp_path, readonly_file_source
):
    from molt.file_hashing import open_stable_read_descriptor

    source, attributes = readonly_file_source
    before = attributes()
    link = tmp_path / "borrowed.exe"
    link.hardlink_to(source)
    descriptor = open_stable_read_descriptor(link)
    try:
        with pytest.raises(OSError) as raised:
            file_deletion.unlink_file(link)
        assert raised.value.winerror == 32  # ERROR_SHARING_VIOLATION
        assert link.exists()
        assert attributes() == before
        assert source.read_bytes() == b"external source must survive cleanup"
    finally:
        os.close(descriptor)
    file_deletion.unlink_file(link)
    assert not link.exists()
    assert attributes() == before


@pytest.mark.skipif(
    os.name != "nt", reason="actual Windows verbatim filesystem entries"
)
@pytest.mark.parametrize("suffix", [".", " "])
@pytest.mark.parametrize("ancestor", [False, True])
@pytest.mark.parametrize("owner_kind", ["tree", "allocation"])
def test_recursive_deletion_keeps_verbatim_child_names(
    tmp_path, readonly_file_source, suffix, ancestor, owner_kind
):
    from molt.temporary_artifacts import OwnedTemporaryDirectory

    # The hardlink's external source and its live attribute observation remain
    # independent of deletion. The owned tree includes ordinary and verbatim
    # siblings, exactly as real tool-image admission fixtures can create them.
    source, attributes = readonly_file_source
    original_attributes = attributes()
    owner = OwnedTemporaryDirectory(prefix="verbatim-", dir=tmp_path)
    tree = Path(owner.name)
    ordinary_directory = tree / "tools"
    ordinary_directory.mkdir()
    ordinary = ordinary_directory / "tool.exe"
    ordinary.write_bytes(b"ordinary sibling")
    if ancestor:
        special_directory = Path("\\\\?\\" + str(ordinary_directory) + suffix)
        special_directory.mkdir()
        selected = special_directory / "tool.exe"
    else:
        selected = Path("\\\\?\\" + str(ordinary) + suffix)
    os.link(source, selected)
    assert selected.read_bytes() == b"external source must survive cleanup"
    assert ordinary.read_bytes() == b"ordinary sibling"
    if owner_kind == "tree":
        ok, error = file_deletion.delete_path(tree)
        assert ok, error
        owner.cleanup()
    else:
        owner.cleanup()
    assert not tree.exists()
    assert not selected.exists()
    assert source.read_bytes() == b"external source must survive cleanup"
    assert attributes() == original_attributes


@pytest.mark.skipif(
    os.name != "nt", reason="actual Windows verbatim filesystem entries"
)
@pytest.mark.parametrize("suffix", [".", " "])
def test_verbatim_subtree_deletion_preserves_ordinary_sibling(tmp_path, suffix):
    ordinary = tmp_path / "owned"
    ordinary.mkdir()
    (ordinary / "keep").write_bytes(b"outside selected entry")
    selected = Path("\\\\?\\" + str(ordinary) + suffix)
    selected.mkdir()
    (selected / "remove").write_bytes(b"owned selected entry")
    assert file_deletion.delete_path(selected) == (True, "")
    assert not selected.exists()
    assert (ordinary / "keep").read_bytes() == b"outside selected entry"
