import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner


def test_wasm_set_ops_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "set_ops.py"
    src.write_text(
        textwrap.dedent(
            """\
            def show(label, value):
                print(f"{label} {value}")


            def show_set(label, value):
                print(f"{label} {sorted(value)}")


            def show_err(label, func):
                try:
                    func()
                except Exception as exc:
                    print(f"{label} {type(exc).__name__} {exc}")


            s = {1, 2}
            show_set("set_union", s.union({2, 3}))
            show_set("set_union_multi", s.union({3}, [4, 1]))
            show_set("set_intersection", s.intersection({2, 4}))
            show_set("set_intersection_multi", s.intersection({1, 2, 3}, [2, 5]))
            show_set("set_difference", s.difference([2, 5]))
            show_set("set_difference_multi", s.difference({2}, [1]))
            show_set("set_symdiff", s.symmetric_difference({2, 3}))
            show("set_isdisjoint", s.isdisjoint({3, 4}))
            show("set_issubset", s.issubset({1, 2, 3}))
            show("set_issuperset", s.issuperset({1}))
            show("set_copy_is", s.copy() is s)

            s_update = {1, 2}
            show("set_update_ret", s_update.update([2, 3], {4}))
            show_set("set_update_after", s_update)

            s_inter = {1, 2, 3}
            show("set_intersection_update_ret", s_inter.intersection_update({2, 3}, [3, 4]))
            show_set("set_intersection_update_after", s_inter)

            s_diff = {1, 2, 3}
            show("set_difference_update_ret", s_diff.difference_update({2}, [3]))
            show_set("set_difference_update_after", s_diff)

            s_sym = {1, 2, 3}
            show("set_symdiff_update_ret", s_sym.symmetric_difference_update({2, 4}))
            show_set("set_symdiff_update_after", s_sym)

            s_clear = {1, 2}
            show("set_clear_ret", s_clear.clear())
            show("set_clear_len", len(s_clear))

            show_err("set_union_kw", lambda: s.union(bad=1))
            show_err("set_symdiff_0", lambda: s.symmetric_difference())
            show_err("set_symdiff_2", lambda: s.symmetric_difference(1, 2))
            show_err("set_copy_1", lambda: s.copy(1))
            show_err("set_clear_1", lambda: s.clear(1))
            show_err("set_union_wrong_self_list", lambda: set.union([1], {2}))
            show_err("set_union_wrong_self_frozen", lambda: set.union(frozenset({1}), {2}))

            fs = frozenset([1, 2])
            show_set("frozenset_union", fs.union({2, 3}))
            show_set("frozenset_union_multi", fs.union({3}, [4, 1]))
            show_set("frozenset_intersection", fs.intersection({2, 4}))
            show_set("frozenset_intersection_multi", fs.intersection({1, 2, 3}, [2, 5]))
            show_set("frozenset_difference", fs.difference([2, 5]))
            show_set("frozenset_difference_multi", fs.difference({2}, [1]))
            show_set("frozenset_symdiff", fs.symmetric_difference({2, 3}))
            show("frozenset_isdisjoint", fs.isdisjoint({3, 4}))
            show("frozenset_issubset", fs.issubset({1, 2, 3}))
            show("frozenset_issuperset", fs.issuperset({1}))
            show("frozenset_copy_is", fs.copy() is fs)
            show_err("frozenset_union_kw", lambda: fs.union(bad=1))
            show_err("frozenset_symdiff_0", lambda: fs.symmetric_difference())
            show_err("frozenset_union_wrong_self_set", lambda: frozenset.union({1}, {2}))
            """
        )
    )

    expected = textwrap.dedent(
        """\
        set_union [1, 2, 3]
        set_union_multi [1, 2, 3, 4]
        set_intersection [2]
        set_intersection_multi [2]
        set_difference [1]
        set_difference_multi []
        set_symdiff [1, 3]
        set_isdisjoint True
        set_issubset True
        set_issuperset True
        set_copy_is False
        set_update_ret None
        set_update_after [1, 2, 3, 4]
        set_intersection_update_ret None
        set_intersection_update_after [3]
        set_difference_update_ret None
        set_difference_update_after [1]
        set_symdiff_update_ret None
        set_symdiff_update_after [1, 3, 4]
        set_clear_ret None
        set_clear_len 0
        set_union_kw TypeError set.union() takes no keyword arguments
        set_symdiff_0 TypeError set.symmetric_difference() takes exactly one argument (0 given)
        set_symdiff_2 TypeError set.symmetric_difference() takes exactly one argument (2 given)
        set_copy_1 TypeError set.copy() takes no arguments (1 given)
        set_clear_1 TypeError set.clear() takes no arguments (1 given)
        set_union_wrong_self_list TypeError descriptor 'union' for 'set' objects doesn't apply to a 'list' object
        set_union_wrong_self_frozen TypeError descriptor 'union' for 'set' objects doesn't apply to a 'frozenset' object
        frozenset_union [1, 2, 3]
        frozenset_union_multi [1, 2, 3, 4]
        frozenset_intersection [2]
        frozenset_intersection_multi [2]
        frozenset_difference [1]
        frozenset_difference_multi []
        frozenset_symdiff [1, 3]
        frozenset_isdisjoint True
        frozenset_issubset True
        frozenset_issuperset True
        frozenset_copy_is True
        frozenset_union_kw TypeError frozenset.union() takes no keyword arguments
        frozenset_symdiff_0 TypeError frozenset.symmetric_difference() takes exactly one argument (0 given)
        frozenset_union_wrong_self_set TypeError descriptor 'union' for 'frozenset' objects doesn't apply to a 'set' object
        """
    ).strip()

    output_wasm = tmp_path / "output.wasm"
    runner = write_wasm_runner(tmp_path, "run_wasm_set_ops.js")

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
    assert run.stdout.strip() == expected
