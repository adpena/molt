import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


CALL_SRC = textwrap.dedent(
    """\
    def add13(a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13):
        return (
            a1
            + a2
            + a3
            + a4
            + a5
            + a6
            + a7
            + a8
            + a9
            + a10
            + a11
            + a12
            + a13
        )

    def main():
        args = (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13)
        print(add13(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13))
        fn = add13
        print(fn(*args))

    if __name__ == "__main__":
        main()
    """
)


def test_wasm_call_arity_trampoline(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "call_arity_trampoline.py"
    src.write_text(CALL_SRC)

    output_wasm = build_wasm_linked(
        root,
        src,
        tmp_path,
        extra_args=["--codec", "json"],
    )
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "91\n91"
