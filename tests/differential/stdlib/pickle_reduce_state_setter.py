"""Differential coverage for pickle reducer 6-tuple state_setter semantics."""

from __future__ import annotations

import pickle


def _state_setter(obj: "Stateful", state: dict[str, int]) -> None:
    obj.value = state["value"] + 1
    obj.trace.append("state_setter")


class Stateful:
    def __init__(self, value: int) -> None:
        self.value = value
        self.trace: list[str] = []

    def __reduce_ex__(self, protocol: int):
        return (Stateful, (0,), {"value": self.value}, None, None, _state_setter)


def _roundtrip(proto: int) -> tuple[int, list[str]]:
    obj = Stateful(41)
    restored = pickle.loads(pickle.dumps(obj, protocol=proto))
    return restored.value, restored.trace


def main() -> None:
    print("proto2", _roundtrip(2))
    print("proto5", _roundtrip(5))


if __name__ == "__main__":
    main()
