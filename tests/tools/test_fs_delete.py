"""Cross-platform teeth for the canonical artifact deletion primitive."""

from __future__ import annotations

import stat
from pathlib import Path

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


def test_retry_readonly_clears_owner_write_bit_before_retry(tmp_path: Path) -> None:
    readonly = tmp_path / "object"
    readonly.write_bytes(b"x")
    readonly.chmod(stat.S_IREAD)
    retried: list[Path] = []

    def remove(raw_path: str) -> None:
        path = Path(raw_path)
        assert path.stat().st_mode & stat.S_IWUSR
        retried.append(path)
        path.unlink()

    file_deletion._retry_readonly(remove, str(readonly), PermissionError("read-only"))

    assert retried == [readonly]
    assert not readonly.exists()


def test_retry_readonly_does_not_mask_non_permission_failures(tmp_path: Path) -> None:
    error = OSError("invalid filesystem operation")
    with pytest.raises(OSError) as raised:
        file_deletion._retry_readonly(lambda _path: None, str(tmp_path), error)
    assert raised.value is error
