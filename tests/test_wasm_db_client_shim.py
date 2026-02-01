import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner

HEADER_BYTES = """const DB_HEADER_BYTES = Uint8Array.from([
  0x83,
  0xa6, 0x73, 0x74, 0x61, 0x74, 0x75, 0x73,
  0xa2, 0x6f, 0x6b,
  0xa5, 0x63, 0x6f, 0x64, 0x65, 0x63,
  0xa7, 0x6d, 0x73, 0x67, 0x70, 0x61, 0x63, 0x6b,
  0xa7, 0x70, 0x61, 0x79, 0x6c, 0x6f, 0x61, 0x64,
  0xc4, 0x01, 0x7f,
]);
"""

IMPORT_OVERRIDES = """
  db_query_obj: (payloadBits, _token) => {
    const data = getBytes(payloadBits) || getBytearray(payloadBits);
    if (!data) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'send expects bytes-like object' })),
      );
      return raiseException(exc);
    }
    const stream = streamCreate();
    if (stream.obj) {
      stream.obj.queue.push(DB_HEADER_BYTES);
      stream.obj.closed = true;
    }
    return stream.handle;
  },
  msgpack_parse_scalar_obj: (bits) => {
    const obj = getObj(bits);
    if (!obj || (obj.type !== 'bytes' && obj.type !== 'bytearray')) return boxNone();
    const dictBits = baseImports.dict_new();
    baseImports.dict_set(
      dictBits,
      boxPtr({ type: 'str', value: 'status' }),
      boxPtr({ type: 'str', value: 'ok' }),
    );
    baseImports.dict_set(
      dictBits,
      boxPtr({ type: 'str', value: 'codec' }),
      boxPtr({ type: 'str', value: 'msgpack' }),
    );
    baseImports.dict_set(
      dictBits,
      boxPtr({ type: 'str', value: 'payload' }),
      boxPtr({ type: 'bytes', data: Uint8Array.from([0x7f]) }),
    );
    return dictBits;
  },
"""


def test_wasm_db_client_shim_msgpack(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "db_client_shim.py"
    src.write_text(
        textwrap.dedent(
            """\
            import asyncio
            from molt import molt_db

            async def main():
                resp = await molt_db.db_query(b"request")
                print(resp.status)
                print(resp.codec)
                print(resp.payload)

            asyncio.run(main())
            """
        )
    )

    output_wasm = tmp_path / "output.wasm"
    runner = write_wasm_runner(
        tmp_path,
        "run_wasm_db_client_shim.js",
        extra_js=HEADER_BYTES,
        import_overrides=IMPORT_OVERRIDES,
    )

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    build = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src),
            "--target",
            "wasm",
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    run = subprocess.run(
        ["node", str(runner), str(output_wasm)],
        cwd=root,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    expected = "ok\nmsgpack\nb'\\x7f'"
    assert run.stdout.strip() == expected
