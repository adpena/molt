"""Tests for wasm_bundle.py."""
from __future__ import annotations
import json
import tarfile
from pathlib import Path
import importlib.util

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
    (src / "main.py").write_text("print('hello')")
    (src / "lib.py").write_text("X = 1")

    output = tmp_path / "bundle.tar"
    manifest = bundle_mod.create_bundle(src, output)

    assert output.exists()
    assert len(manifest["files"]) == 2
    assert manifest["total_bytes"] > 0

    with tarfile.open(output) as tar:
        names = tar.getnames()
        assert "main.py" in names
        assert "lib.py" in names
        assert "__manifest__.json" in names

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
    (src / "pkg" / "mod.py").write_text("Y = 2")

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
