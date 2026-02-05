from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_list_dict_ops_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "list_dict_ops.py"
    src.write_text(
        "lst = [1, 2, 3]\n"
        "lst.append(4)\n"
        "print(lst[0])\n"
        "print(lst[-1])\n"
        "print(len(lst[1:3]))\n"
        "print(lst.pop())\n"
        "print(lst.pop(0))\n"
        "d = {1: 10, 2: 20}\n"
        "print(d.get(1))\n"
        "print(d.get(3))\n"
        "print(d.get(3, 99))\n"
        "d[3] = 30\n"
        "ks = d.keys()\n"
        "vs = d.values()\n"
        "print(len(ks))\n"
        "print(ks[1])\n"
        "print(vs[2])\n"
        "lst2 = [1, 2, 3]\n"
        "lst2.extend([4, 5])\n"
        "lst2.insert(1, 99)\n"
        "lst2.remove(2)\n"
        "print(lst2[0])\n"
        "print(lst2[1])\n"
        "print(len(lst2))\n"
        "lst2.remove(99)\n"
        "print(len(lst2))\n"
        "t = (1, 2, 1)\n"
        "print(t.count(1))\n"
        "print(t.index(2))\n"
        "print(t.index(9))\n"
        "d2 = {1: 10, 2: 20}\n"
        "print(d2.pop(1))\n"
        "print(d2.pop(3, 99))\n"
        "items = d2.items()\n"
        "print(len(items))\n"
        "print(items[0][0])\n"
        "print(items[0][1])\n"
        "total = 0\n"
        "for x in [1, 2, 3]:\n"
        "    total = total + x\n"
        "print(total)\n"
        "acc = 0\n"
        "for x in (4, 5):\n"
        "    acc = acc + x\n"
        "print(acc)\n"
        "d3 = {7: 70, 8: 80}\n"
        "sumk = 0\n"
        "for x in d3.keys():\n"
        "    sumk = sumk + x\n"
        "print(sumk)\n"
        "sumv = 0\n"
        "for x in d3.values():\n"
        "    sumv = sumv + x\n"
        "print(sumv)\n"
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert (
        run.stdout.strip()
        == "1\n4\n2\n4\n1\n10\nNone\n99\n3\n2\n30\n1\n99\n5\n4\n2\n1\nNone\n10\n99\n1\n2\n20\n6\n9\n15\n150"
    )
