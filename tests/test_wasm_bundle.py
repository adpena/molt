"""Tests for wasm_bundle.py."""

from __future__ import annotations
import json
import os
import tarfile
from pathlib import Path
import importlib.util

import pytest

from molt import artifact_publication
from molt.wasm_bundle import write_wasm_bundle

PROJECT_ROOT = Path(__file__).resolve().parents[1]


def _load_bundle_module():
    path = PROJECT_ROOT / "tools" / "wasm_bundle.py"
    spec = importlib.util.spec_from_file_location("wasm_bundle", path)
    assert spec is not None and spec.loader is not None
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


bundle_mod = _load_bundle_module()


def test_create_bundle_basic(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    artifact_publication.atomic_write_text(src / "main.py", "print('hello')")
    artifact_publication.atomic_write_text(src / "lib.py", "X = 1")

    output = tmp_path / "bundle.tar"
    manifest = bundle_mod.create_bundle(src, output)

    assert output.exists()
    assert len(manifest["files"]) == 2
    assert manifest["total_bytes"] > 0

    with tarfile.open(output) as tar:
        names = tar.getnames()
        assert set(names) == {"main.py", "lib.py", "__manifest__.json"}


def test_bundle_rejects_malformed_publication_lock(tmp_path):
    src = tmp_path / "src"
    artifact_publication.atomic_write_text(src / "main.py", "pass")
    lock = next(path for path in src.iterdir() if path.name != "main.py")
    lock.write_bytes(b"not lock metadata")
    with pytest.raises(ValueError, match="lock must be empty"):
        bundle_mod.create_bundle(src, tmp_path / "bundle.tar")


def test_bundle_skips_pycache(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    (src / "main.py").write_text("pass")
    cache = src / "__pycache__"
    cache.mkdir()
    (cache / "main.cpython-312.pyc").write_bytes(b"compiled")

    output = tmp_path / "bundle.tar"
    manifest = bundle_mod.create_bundle(src, output)

    assert len(manifest["files"]) == 1
    assert all("__pycache__" not in f["path"] for f in manifest["files"])


def test_bundle_includes_subdirectories(tmp_path):
    src = tmp_path / "src"
    (src / "pkg").mkdir(parents=True)
    (src / "pkg" / "__init__.py").write_text("")
    artifact_publication.atomic_write_text(src / "pkg" / "mod.py", "Y = 2")

    output = tmp_path / "bundle.tar"
    manifest = bundle_mod.create_bundle(src, output)

    paths = [f["path"] for f in manifest["files"]]
    assert "pkg/__init__.py" in paths
    assert "pkg/mod.py" in paths


def test_bundle_manifest_is_valid_json(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    (src / "app.py").write_text("pass")

    output = tmp_path / "bundle.tar"
    bundle_mod.create_bundle(src, output)

    with tarfile.open(output) as tar:
        manifest_data = tar.extractfile("__manifest__.json").read()
        manifest = json.loads(manifest_data)
        assert "files" in manifest
        assert "total_bytes" in manifest


def test_bundle_is_deterministic_and_never_packages_private_stages(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    source = src / "app.py"
    source.write_bytes(b"pass")
    stage = artifact_publication.staged_output_path(source)
    stage.write_bytes(b"uncommitted")
    output = tmp_path / "bundle.tar"
    bundle_mod.create_bundle(src, output)
    expected = output.read_bytes()
    os.utime(source, (42, 42))
    bundle_mod.create_bundle(src, output)
    assert output.read_bytes() == expected
    with tarfile.open(output) as archive:
        assert archive.getnames() == ["app.py", "__manifest__.json"]


def test_bundle_changed_source_preserves_previous_archive(tmp_path, monkeypatch):
    src = tmp_path / "src"
    src.mkdir()
    source = src / "app.py"
    source.write_bytes(b"before")
    output = tmp_path / "bundle.tar"
    output.write_bytes(b"previous archive")
    original_add = tarfile.TarFile.addfile

    def mutate_after_read(archive, info, fileobj=None):
        original_add(archive, info, fileobj)
        if info.name == "app.py":
            source.write_bytes(b"after!")

    monkeypatch.setattr(tarfile.TarFile, "addfile", mutate_after_read)
    with pytest.raises(ValueError, match="changed"):
        bundle_mod.create_bundle(src, output)
    assert output.read_bytes() == b"previous archive"
    assert sorted(path.name for path in tmp_path.iterdir()) == ["bundle.tar", "src"]


def test_bundle_order_and_metadata_do_not_depend_on_host_or_root_order(tmp_path):
    roots = (tmp_path / "z-checkout", tmp_path / "a-checkout")
    for root, name in zip(roots, ("Z.exe", "a.py"), strict=True):
        root.mkdir()
        (root / name).write_bytes(b"payload")
    first, second = tmp_path / "first.tar", tmp_path / "second.tar"
    write_wasm_bundle(roots, first)
    for root in roots:
        for path in root.iterdir():
            path.chmod(0o755)
            os.utime(path, (42, 42))
    write_wasm_bundle(reversed(roots), second)
    assert first.read_bytes() == second.read_bytes()
    with tarfile.open(first) as archive:
        assert archive.getnames() == ["Z.exe", "a.py", "__manifest__.json"]
        assert all(
            member.mode == 0o644
            and member.mtime == member.uid == member.gid == 0
            and member.uname == member.gname == ""
            for member in archive.getmembers()
        )


@pytest.mark.parametrize(
    "names",
    [
        ("pkg.py", "PKG.py"),
        ("__manifest__.json", "other.py"),
        ("pkg", "pkg/module.py"),
        ("caf\u00e9.py", "cafe\u0301.py"),
    ],
)
def test_bundle_rejects_cross_root_portable_collisions_without_replacing_output(
    tmp_path, names
):
    roots = (tmp_path / "left", tmp_path / "right")
    for root, name in zip(roots, names, strict=True):
        path = root / name
        path.parent.mkdir(parents=True)
        path.write_bytes(b"payload")
    output = tmp_path / "previous.tar"
    output.write_bytes(b"previous generation")
    with pytest.raises(ValueError, match="collision"):
        write_wasm_bundle(roots, output)
    assert output.read_bytes() == b"previous generation"
