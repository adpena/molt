"""Differential coverage for pickle class/dataclass roundtrip edge semantics."""

from __future__ import annotations

import dataclasses
import pickle


class Node:
    def __init__(self, value: int) -> None:
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
    def __new__(cls, *, value: int):
        obj = super().__new__(cls)
        obj.value = value
        return obj

    def __getnewargs_ex__(self):
        return (), {"value": self.value}


def main() -> None:
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


if __name__ == "__main__":
    main()
