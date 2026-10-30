from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    molt_worker_stub_env,
    require_wasm_toolchain,
    run_wasm_linked,
)

# A synchronous molt_stream_recv on WASM drives the host's database poll
# (molt_db_host_poll) before it reports a pending stream, so a plain receive
# loop, with no event loop, collects the worker's reply. The program reaches
# the stream intrinsics through runtime admission (require_intrinsic); bare
# intrinsic names stopped being builtins in 8a74a534a3. The worker stub
# answers db_query with msgpack 127 (0x7f), which the host frames into the
# stream's msgpack header.
PROGRAM = """\
import time
from _intrinsics import require_intrinsic

query = require_intrinsic("molt_db_query_obj")
recv = require_intrinsic("molt_stream_recv")
drop = require_intrinsic("molt_stream_drop")
parse = require_intrinsic("molt_msgpack_parse_scalar_obj")


def main():
    stream = query(b"request", None)
    deadline = time.monotonic() + 60.0
    while True:
        data = recv(stream)
        if isinstance(data, (bytes, bytearray)):
            header = parse(bytes(data))
            print(header["status"])
            print(header["codec"])
            print(header["payload"])
            break
        if time.monotonic() >= deadline:
            print("pending")
            break
        time.sleep(0.001)
    drop(stream)


if __name__ == "__main__":
    main()
"""


def test_wasm_db_host_poll_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "db_host_poll.py"
    src.write_text(PROGRAM, encoding="utf-8")

    # Deployment manifest v4: the build's launch policy owns the grant.
    output_wasm = build_wasm_linked(
        root, src, tmp_path, extra_args=["--capabilities", "db.read"]
    )
    run = run_wasm_linked(root, output_wasm, env_overrides=molt_worker_stub_env(root))
    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["ok", "msgpack", "b'\\x7f'"]
