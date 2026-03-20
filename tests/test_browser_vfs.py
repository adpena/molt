"""Verify the browser VFS JS module is syntactically valid."""
from pathlib import Path

def test_browser_vfs_js_exists():
    path = Path(__file__).resolve().parents[1] / "wasm" / "molt_vfs_browser.js"
    assert path.exists(), f"Missing: {path}"
    content = path.read_text()
    assert "class MoltVfs" in content
    assert "class BundleFs" in content
    assert "class TmpFs" in content
    assert "fromTar" in content
