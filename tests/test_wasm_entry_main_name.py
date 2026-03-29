import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


ENTRY_MAIN_NAME_SRC = textwrap.dedent(
    """\
    print(__name__)

    if __name__ == "__main__":
        print("guard")
    """
)


def test_wasm_linked_entry_module_uses_main_name(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "entry_name.py"
    src.write_text(ENTRY_MAIN_NAME_SRC)

    output_wasm = build_wasm_linked(
        root,
        src,
        tmp_path,
        extra_args=["--codec", "json"],
    )
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "__main__\nguard"
