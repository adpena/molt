import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    molt_worker_stub_env,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_db_client_shim_msgpack(tmp_path: Path) -> None:
    require_wasm_toolchain()

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
        ),
        encoding="utf-8",
    )

    # Deployment manifest v4: the build's launch policy owns the grant.
    output_wasm = build_wasm_linked(
        root, src, tmp_path, extra_args=["--capabilities", "db.read"]
    )
    # The worker stub answers db_query with msgpack 127 (0x7f): the host's
    # framing and the shim's decoding must deliver exactly that response.
    run = run_wasm_linked(root, output_wasm, env_overrides=molt_worker_stub_env(root))
    assert run.returncode == 0, run.stderr
    expected = "ok\nmsgpack\nb'\\x7f'"
    assert run.stdout.strip() == expected
