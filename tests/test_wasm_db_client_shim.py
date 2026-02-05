import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
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
        )
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    expected = "ok\nmsgpack\nb'\\x7f'"
    assert run.stdout.strip() == expected
