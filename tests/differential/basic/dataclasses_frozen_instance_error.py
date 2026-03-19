from dataclasses import FrozenInstanceError, dataclass


@dataclass(frozen=True)
class FrozenPoint:
    x: int


fp = FrozenPoint(1)

try:
    fp.x = 2
except FrozenInstanceError:
    print("assign", "FrozenInstanceError")
except Exception as exc:
    print("assign", type(exc).__name__)

try:
    del fp.x
except FrozenInstanceError:
    print("delete", "FrozenInstanceError")
except Exception as exc:
    print("delete", type(exc).__name__)
