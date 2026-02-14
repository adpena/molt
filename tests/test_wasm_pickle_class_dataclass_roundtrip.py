from __future__ import annotations

import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_pickle_class_dataclass_roundtrip(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "pickle_class_dataclass_roundtrip.py"
    src.write_text(
        textwrap.dedent(
            """\
            import dataclasses
            import pickle


            class Node:
                def __init__(self, value):
                    self.value = value
                    self.next = None


            @dataclasses.dataclass
            class Plain:
                x: int
                y: int = 2


            @dataclasses.dataclass(slots=True)
            class Slots:
                x: int
                y: int = 2


            @dataclasses.dataclass(slots=True, frozen=True)
            class FrozenSlots:
                x: int
                y: int = 2


            class KwOnlyNew:
                def __new__(cls, *, value):
                    obj = super().__new__(cls)
                    obj.value = value
                    return obj

                def __getnewargs_ex__(self):
                    return (), {"value": self.value}


            node = Node(1)
            node.next = node
            out = pickle.loads(pickle.dumps(node, protocol=5))
            print("node_cycle", out is out.next, out.value)

            plain = Plain(3, 4)
            plain_out = pickle.loads(pickle.dumps(plain, protocol=5))
            print("plain", plain_out, type(plain_out) is Plain)

            slots = Slots(5, 6)
            slots_out = pickle.loads(pickle.dumps(slots, protocol=5))
            print("slots", slots_out, type(slots_out) is Slots)

            frozen = FrozenSlots(7, 8)
            frozen_out = pickle.loads(pickle.dumps(frozen, protocol=5))
            print("frozen_slots", frozen_out, type(frozen_out) is FrozenSlots)

            kw = KwOnlyNew(value=9)
            kw_out = pickle.loads(pickle.dumps(kw, protocol=5))
            print("kw_only_new", kw_out.value, type(kw_out) is KwOnlyNew)
            """
        )
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == (
        "node_cycle True 1\n"
        "plain Plain(x=3, y=4) True\n"
        "slots Slots(x=5, y=6) True\n"
        "frozen_slots FrozenSlots(x=7, y=8) True\n"
        "kw_only_new 9 True"
    )
