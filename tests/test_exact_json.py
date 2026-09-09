from __future__ import annotations

import math
import hashlib
import os
from pathlib import Path

import pytest

from molt import exact_json, file_publication


def test_exact_file_read_admits_byte_limit_and_rejects_before_decode(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "receipt.json"
    path.write_bytes(b'{"value":1}')
    assert exact_json.read_exact(path, max_bytes=11, label="test receipt") == {
        "value": 1
    }
    monkeypatch.setattr(
        exact_json,
        "loads_exact",
        lambda _raw: pytest.fail("oversized bytes reached JSON parser"),
    )
    with pytest.raises(
        exact_json.ExactJsonError, match="test receipt exceeds size limit"
    ):
        exact_json.read_exact(path, max_bytes=10, label="test receipt")


@pytest.mark.parametrize("raw", (b'{"v":1,"v":2}', b'{"v":NaN}', b'"\xff"'))
def test_exact_file_read_preserves_strict_codec(tmp_path: Path, raw: bytes) -> None:
    path = tmp_path / "receipt.json"
    path.write_bytes(raw)
    with pytest.raises((ValueError, UnicodeError)):
        exact_json.read_exact(path, max_bytes=100, label="test receipt")


@pytest.mark.parametrize("limit", (0, -1, True, 1.5))
def test_exact_file_read_rejects_invalid_budget_before_open(
    tmp_path: Path, limit
) -> None:
    with pytest.raises(ValueError, match="positive integer"):
        exact_json.read_exact(
            tmp_path / "missing", max_bytes=limit, label="test receipt"
        )


@pytest.mark.parametrize(
    "payload",
    (
        '{"outer":{"key":1,"key":2}}',
        '{"key":1,"key":2}',
    ),
)
def test_loads_exact_rejects_duplicate_keys_at_every_depth(payload: str) -> None:
    with pytest.raises(exact_json.ExactJsonError, match="duplicate JSON key 'key'"):
        exact_json.loads_exact(payload)


@pytest.mark.parametrize(
    "token",
    ("NaN", "Infinity", "-Infinity", "1e9999", "-1e9999", "1.7976931348623159e308"),
)
def test_loads_exact_rejects_every_non_finite_number(token: str) -> None:
    with pytest.raises(exact_json.ExactJsonError, match="non-finite JSON number"):
        exact_json.loads_exact(f'{{"value":{token}}}')


@pytest.mark.parametrize(
    "token", ["1.7976931348623157e308", "5e-324", "-0.0", "1e-9999"]
)
def test_exact_json_preserves_finite_float_boundaries(token: str) -> None:
    actual = exact_json.loads_exact(token)
    expected = float(token)
    assert actual == expected
    assert math.copysign(1.0, actual) == math.copysign(1.0, expected)


def test_exact_encoding_is_deterministic_utf8_and_finite(tmp_path: Path) -> None:
    expected = '{\n  "a": "café",\n  "z": 1\n}\n'.encode()
    assert exact_json.encode_exact({"z": 1, "a": "café"}) == expected
    assert exact_json.dumps_exact({"z": 1, "a": "café"}, indent=None) == (
        '{"a":"café","z":1}\n'
    )
    assert exact_json.canonical_json_bytes({"z": 1, "a": "café"}) == (
        '{"a":"café","z":1}'.encode()
    )
    assert exact_json.canonical_json_sha256({"z": 1, "a": "café"}) == (
        "79ab3e11fc70c4b67c474b34ff77941ed7eba4d33b123ac3e2bedd2e98dbe2bc"
    )
    with pytest.raises(ValueError, match="Out of range float values"):
        exact_json.encode_exact({"value": math.nan})
    with pytest.raises(ValueError, match="Out of range float values"):
        exact_json.canonical_json_bytes({"value": math.nan})
    with pytest.raises(ValueError, match="Out of range float values"):
        exact_json.canonical_json_sha256({"value": math.nan})

    path = tmp_path / "nested" / "identity.json"
    exact_json.write_exact(path, {"z": 1, "a": "café"}, exclusive=True)
    assert path.read_bytes() == expected
    with pytest.raises(FileExistsError):
        exact_json.write_exact(path, {}, exclusive=True)


def test_exact_write_failure_preserves_public_bytes_and_reaps_stage(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "identity.json"
    path.write_bytes(b"prior\n")

    def fail_before_commit(staged: Path, destination: Path) -> None:
        assert staged.read_bytes().startswith(b"{")
        assert destination.read_bytes() == b"prior\n"
        raise OSError("injected namespace failure")

    monkeypatch.setattr(file_publication, "durable_replace", fail_before_commit)

    with pytest.raises(OSError, match="injected namespace failure"):
        exact_json.write_exact(path, {"generation": 2})

    assert path.read_bytes() == b"prior\n"
    assert tuple(tmp_path.iterdir()) == (path,)


def test_exclusive_publication_never_replaces_an_existing_leaf(
    tmp_path: Path,
) -> None:
    staged = tmp_path / "staged"
    destination = tmp_path / "destination"
    staged.write_bytes(b"candidate")
    destination.write_bytes(b"prior")

    with pytest.raises(FileExistsError):
        file_publication.durable_publish_exclusive(staged, destination)

    assert staged.read_bytes() == b"candidate"
    assert destination.read_bytes() == b"prior"


@pytest.mark.parametrize("dangling", [False, True])
def test_owned_path_rejects_ancestor_links_before_resolution(
    tmp_path: Path,
    dangling: bool,
) -> None:
    real = tmp_path / "real"
    if not dangling:
        real.mkdir()
    link = tmp_path / "alias"
    try:
        link.symlink_to(real, target_is_directory=True)
    except OSError:
        pytest.skip("host does not permit creating directory symlinks")
    for path in (link / "child", link / ".." / "child"):
        with pytest.raises(ValueError, match="link or junction"):
            file_publication.resolve_owned_path(path)
    assert not (real / "child").exists()


def test_staging_names_are_bounded_unique_and_destination_bound(tmp_path: Path) -> None:
    destination = tmp_path / ("authority-" * 24 + ".json")
    first = file_publication.staged_file_path(destination)
    second = file_publication.staged_file_path(destination)
    expected = hashlib.sha256(os.fsencode(destination.name)).hexdigest()[:16]
    assert first.parent == second.parent == destination.parent
    assert first != second
    assert first.name.startswith(f".molt-write-{expected}-")
    assert first.name.endswith(".tmp")
    assert len(first.name) == len(".molt-write--.tmp") + 16 + 32
    assert not first.exists() and not second.exists()
    nested = first
    for _ in range(8):
        previous = nested
        nested = file_publication.staged_file_path(previous)
        assert nested.parent == destination.parent
        assert len(nested.name) == len(first.name)
        assert nested != previous
        assert not nested.exists()


@pytest.mark.parametrize("parent_traversal", [False, True])
def test_owned_cleanup_rejects_resolved_filesystem_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, parent_traversal: bool
) -> None:
    root = Path(tmp_path.anchor)
    supplied = root / "molt-missing-child" / ".." if parent_traversal else root

    def refuse_delete(path: Path) -> None:
        raise AssertionError(f"root must be rejected before deletion: {path}")

    monkeypatch.setattr(file_publication.shutil, "rmtree", refuse_delete)
    with pytest.raises(ValueError, match="real owned leaf"):
        file_publication.durable_remove_path(supplied)


def test_owned_cleanup_and_quarantine_reject_ancestor_links(tmp_path: Path) -> None:
    real = tmp_path / "real"
    evidence = real / "evidence"
    evidence.mkdir(parents=True)
    (evidence / "payload").write_bytes(b"preserve")
    link = tmp_path / "alias"
    try:
        link.symlink_to(real, target_is_directory=True)
    except OSError:
        pytest.skip("host does not permit creating directory symlinks")
    with pytest.raises(ValueError, match="link or junction"):
        file_publication.durable_remove_path(link / "evidence")
    with pytest.raises(ValueError, match="link or junction"):
        file_publication.durable_namespace_publish_directory_exclusive(
            link / "evidence", tmp_path / "quarantine"
        )
    with pytest.raises(ValueError, match="link or junction"):
        file_publication.durable_namespace_publish_directory_exclusive(
            evidence, link / "quarantine"
        )
    assert (evidence / "payload").read_bytes() == b"preserve"


@pytest.mark.parametrize("kind", ["file", "directory"])
def test_retirement_moves_same_parent_before_reclamation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kind: str
) -> None:
    source = tmp_path / ("long-name-" * 15)
    if kind == "file":
        source.write_bytes(b"retire")
    else:
        source.mkdir()
        (source / "journal").write_bytes(b"retire")
    calls = []
    rename = file_publication._namespace_publish_leaf_exclusive_once

    def observe_rename(left, right):
        assert left == source
        assert right.parent == source.parent
        assert len(right.name) < 255
        calls.append((left, right))
        rename(left, right)
        assert not left.exists() and right.exists()

    monkeypatch.setattr(
        file_publication, "_namespace_publish_leaf_exclusive_once", observe_rename
    )
    file_publication.durable_remove_path(source, retirement_scope="owned")
    assert len(calls) == 1
    assert list(tmp_path.iterdir()) == []


def test_retirement_barrier_failure_retains_intact_payload_and_new_live_name(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "transaction"
    source.mkdir()
    (source / "journal").write_bytes(b"must survive until durable")
    sync = file_publication.fsync_directory

    def fail_after_rename(parent):
        if not source.exists():
            raise OSError("injected retirement barrier")
        sync(parent)

    with monkeypatch.context() as faults:
        faults.setattr(file_publication, "fsync_directory", fail_after_rename)
        with pytest.raises(file_publication.RetirementError) as caught:
            file_publication.durable_remove_path(source, retirement_scope="owned")
    retired = caught.value.retired_path
    assert caught.value.namespace_committed
    assert (retired / "journal").read_bytes() == b"must survive until durable"
    source.mkdir()
    (source / "new-live").write_bytes(b"do not remove")
    assert file_publication.reclaim_retired_paths(
        tmp_path, retirement_scope="owned"
    ) == (retired,)
    assert (source / "new-live").read_bytes() == b"do not remove"


def test_retirement_recovery_rejects_substituted_identity_and_preserves_other_scope(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "transaction"
    source.mkdir()
    (source / "journal").write_bytes(b"old")
    with monkeypatch.context() as faults:

        def partial_remove(path):
            (path / "journal").unlink()
            raise OSError("injected after journal removal")

        faults.setattr(file_publication.shutil, "rmtree", partial_remove)
        with pytest.raises(file_publication.RetirementError) as caught:
            file_publication.durable_remove_path(source, retirement_scope="owned")
    retired = caught.value.retired_path
    assert not source.exists()
    assert (
        file_publication.reclaim_retired_paths(tmp_path, retirement_scope="unrelated")
        == ()
    )
    preserved = tmp_path / "preserved-original"
    retired.rename(preserved)
    retired.mkdir()
    (retired / "rival").write_bytes(b"preserve")
    with pytest.raises(file_publication.RetirementError, match="identity changed"):
        file_publication.reclaim_retired_paths(tmp_path, retirement_scope="owned")
    assert (retired / "rival").read_bytes() == b"preserve"
    assert preserved.is_dir()


def test_retirement_never_overwrites_destination_collision(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "transaction"
    source.mkdir()
    (source / "journal").write_bytes(b"preserve live")
    rename = file_publication._namespace_publish_leaf_exclusive_once
    rival = []

    def collide(left, right):
        right.mkdir()
        (right / "rival").write_bytes(b"preserve rival")
        rival.append(right)
        rename(left, right)

    monkeypatch.setattr(
        file_publication, "_namespace_publish_leaf_exclusive_once", collide
    )
    with pytest.raises(FileExistsError):
        file_publication.durable_remove_path(source, retirement_scope="owned")
    assert (source / "journal").read_bytes() == b"preserve live"
    assert (rival[0] / "rival").read_bytes() == b"preserve rival"


def test_reclaimed_file_barrier_failure_is_explicit_and_retryable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "file"
    source.write_bytes(b"retire")
    sync = file_publication.fsync_directory
    seen_retirement = False

    def fail_after_delete(parent):
        nonlocal seen_retirement
        if list(parent.glob(".molt-retired-v1-*")):
            seen_retirement = True
        elif seen_retirement:
            raise OSError("injected reclamation barrier")
        sync(parent)

    with monkeypatch.context() as faults:
        faults.setattr(file_publication, "fsync_directory", fail_after_delete)
        with pytest.raises(file_publication.RetirementError) as caught:
            file_publication.durable_remove_path(source, retirement_scope="owned")
    assert caught.value.phase == "reclamation durability"
    assert not source.exists() and not caught.value.retired_path.exists()
    assert (
        file_publication.reclaim_retired_paths(tmp_path, retirement_scope="owned") == ()
    )


def test_retirement_does_not_follow_contained_directory_links(tmp_path: Path) -> None:
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "payload").write_bytes(b"preserve")
    source = tmp_path / "transaction"
    source.mkdir()
    try:
        (source / "link").symlink_to(outside, target_is_directory=True)
    except OSError:
        pytest.skip("host does not permit creating directory symlinks")
    file_publication.durable_remove_path(source, retirement_scope="owned")
    assert (outside / "payload").read_bytes() == b"preserve"


def test_retirement_rejects_unavailable_identity_without_renaming(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "file"
    source.write_bytes(b"preserve")
    lstat = Path.lstat

    def without_identity(path, *args, **kwargs):
        metadata = lstat(path, *args, **kwargs)
        if path == source:
            fields = list(metadata)
            fields[1] = 0
            return os.stat_result(
                fields,
                {
                    field: getattr(metadata, field)
                    for field in ("st_file_attributes", "st_reparse_tag")
                    if hasattr(metadata, field)
                },
            )
        return metadata

    monkeypatch.setattr(Path, "lstat", without_identity)
    with pytest.raises(ValueError, match="stable filesystem file identity"):
        file_publication.durable_remove_path(source, retirement_scope="owned")
    assert source.read_bytes() == b"preserve"
    assert not list(tmp_path.glob(".molt-retired-v1-*"))


def test_retirement_recovery_preserves_malformed_owned_name(tmp_path: Path) -> None:
    malformed = tmp_path / (file_publication._retirement_prefix("owned") + "untyped")
    malformed.mkdir()
    (malformed / "evidence").write_bytes(b"preserve")
    with pytest.raises(ValueError, match="malformed retired leaf"):
        file_publication.reclaim_retired_paths(tmp_path, retirement_scope="owned")
    assert (malformed / "evidence").read_bytes() == b"preserve"


def test_retirement_rejects_windows_reparse_attribute_before_rename(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from types import SimpleNamespace

    source = tmp_path / "reparse-root"
    source.mkdir()
    lstat = Path.lstat

    def reparse_metadata(path, *args, **kwargs):
        metadata = lstat(path, *args, **kwargs)
        if path == source:
            return SimpleNamespace(
                st_mode=metadata.st_mode,
                st_dev=metadata.st_dev,
                st_ino=metadata.st_ino,
                st_file_attributes=0x400,
            )
        return metadata

    monkeypatch.setattr(file_publication, "_WINDOWS_REPARSE_POINT", 0x400)
    monkeypatch.setattr(Path, "lstat", reparse_metadata)
    with pytest.raises(ValueError, match="link or junction"):
        file_publication.durable_remove_path(source, retirement_scope="owned")
    assert not list(tmp_path.glob(".molt-retired-v1-*"))


@pytest.mark.skipif(os.name != "nt", reason="Windows directory junction consumer")
def test_retirement_refuses_root_junction_and_preserves_nested_junction_target(
    tmp_path: Path,
) -> None:
    import subprocess

    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "payload").write_bytes(b"preserve")
    source = tmp_path / "transaction"
    source.mkdir()
    junction = source / "junction"
    subprocess.run(
        ["cmd", "/c", "mklink", "/J", str(junction), str(outside)],
        check=True,
        capture_output=True,
    )
    assert junction.is_junction()
    with pytest.raises(ValueError, match="link or junction"):
        file_publication.durable_remove_path(junction, retirement_scope="owned")
    file_publication.durable_remove_path(source, retirement_scope="owned")
    assert (outside / "payload").read_bytes() == b"preserve"
