"""Verify the browser VFS JS module is syntactically valid."""

import base64
import io
import json
import tarfile
from pathlib import Path

from tests.native_process_guard import run_native_test_process


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
    result = run_native_test_process(
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


def test_browser_vfs_tar_parser_rejects_path_escape_and_links() -> None:
    path = Path(__file__).resolve().parents[1] / "wasm" / "molt_vfs_browser.js"

    def tar_payload(member: tarfile.TarInfo, data: bytes = b"") -> str:
        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode="w") as tar:
            tar.addfile(member, io.BytesIO(data))
        return base64.b64encode(buf.getvalue()).decode("ascii")

    traversal = tarfile.TarInfo("../secret.txt")
    traversal.size = 1
    link = tarfile.TarInfo("linked.txt")
    link.type = tarfile.SYMTYPE
    link.linkname = "target.txt"

    js = f"""
const {{ BundleFs }} = require({json.dumps(str(path))});
const cases = [
  Buffer.from({json.dumps(tar_payload(traversal, b"x"))}, "base64"),
  Buffer.from({json.dumps(tar_payload(link))}, "base64"),
];
const results = [];
for (const payload of cases) {{
  try {{
    BundleFs.fromTar(new Uint8Array(payload));
    results.push("accepted");
  }} catch (error) {{
    results.push(String(error.message));
  }}
}}
console.log(JSON.stringify(results));
"""
    result = run_native_test_process(
        ["node", "-e", js],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    messages = json.loads(result.stdout)
    assert messages[0] == "bundle tar contains '..' component in path: ../secret.txt"
    assert messages[1] == "bundle tar contains link entry: linked.txt"
