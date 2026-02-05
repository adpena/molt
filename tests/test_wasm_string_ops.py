import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_string_ops_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "string_ops.py"
    src.write_text(
        textwrap.dedent(
            """\
            s = 'alpha,beta,gamma'
            print(s.find('beta'))
            print(s.find('beta', 2))
            print(s.startswith('alpha'))
            print(s.startswith('alpha', 0, 5))
            print(s.endswith('gamma'))
            print(s.endswith('gamma', 0, len(s)))
            parts = s.split(',')
            print(len(parts))
            print(parts[1])
            print(','.join(parts))
            print('ha'.replace('a', 'o'))
            print('mississippi'.count('iss'))
            print('mississippi'.count('iss', 1, 6))
            """
        )
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert (
        run.stdout.strip()
        == "6\n6\nTrue\nTrue\nTrue\nTrue\n3\nbeta\nalpha,beta,gamma\nho\n2\n1"
    )
