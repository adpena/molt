import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner


EXTRA_JS = """\
const DB_POLL_PAYLOAD = Uint8Array.from([0x70, 0x6f, 0x6c, 0x6c]);
let dbPollStream = null;
"""

IMPORT_OVERRIDES = """\
  stream_new: (capacity) => {
    const handle = baseImports.stream_new(capacity);
    if (dbPollStream === null) dbPollStream = handle;
    return handle;
  },
  db_host_poll: () => {
    if (!dbPollStream) return 0;
    const stream = streamGet(dbPollStream);
    if (!stream || stream.queue.length > 0 || stream.closed) return 0;
    stream.queue.push(DB_POLL_PAYLOAD);
    return 0;
  },
"""


def test_wasm_db_host_poll_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "db_host_poll.py"
    src.write_text(
        "def main():\n"
        "    stream = molt_stream_new(0)\n"
        "    data = molt_stream_recv(stream)\n"
        "    if isinstance(data, (bytes, bytearray)):\n"
        "        print(data)\n"
        "        print(len(data))\n"
        "    else:\n"
        "        print('pending')\n"
        "\n"
        "if __name__ == '__main__':\n"
        "    main()\n"
    )

    output_wasm = tmp_path / "output.wasm"
    runner = write_wasm_runner(
        tmp_path,
        "run_wasm_db_host_poll.js",
        extra_js=EXTRA_JS,
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
    assert run.stdout.strip() == "\n".join(["b'poll'", "4"])
