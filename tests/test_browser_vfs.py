"""Verify the browser VFS JS module is syntactically valid."""

import base64
import io
import json
import subprocess
import tarfile
from pathlib import Path


def test_browser_vfs_js_exists():
    path = Path(__file__).resolve().parents[1] / "wasm" / "molt_vfs_browser.js"
    assert path.exists(), f"Missing: {path}"
    content = path.read_text()
    assert "class MoltVfs" in content
    assert "class BundleFs" in content
    assert "class TmpFs" in content
    assert "fromTar" in content


def test_browser_vfs_tar_parser_keeps_zero_byte_files(tmp_path: Path) -> None:
    path = Path(__file__).resolve().parents[1] / "wasm" / "molt_vfs_browser.js"
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        empty = tarfile.TarInfo("empty.txt")
        empty.size = 0
        tar.addfile(empty, io.BytesIO(b""))
        data = b"hi"
        hello = tarfile.TarInfo("hello.txt")
        hello.size = len(data)
        tar.addfile(hello, io.BytesIO(data))
    tar_b64 = base64.b64encode(buf.getvalue()).decode("ascii")
    js = f"""
const {{ BundleFs }} = require({json.dumps(str(path))});
const tarBytes = Buffer.from({json.dumps(tar_b64)}, "base64");
const fs = BundleFs.fromTar(new Uint8Array(tarBytes));
console.log(JSON.stringify({{
  emptyExists: fs.exists("empty.txt"),
  emptySize: fs.read("empty.txt").byteLength,
  helloExists: fs.exists("hello.txt"),
  helloText: Buffer.from(fs.read("hello.txt")).toString("utf8"),
}}));
"""
    result = subprocess.run(
        ["node", "-e", js],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert payload == {
        "emptyExists": True,
        "emptySize": 0,
        "helloExists": True,
        "helloText": "hi",
    }
