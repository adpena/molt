"""Purpose: differential coverage for enum basics."""

from enum import Enum, IntEnum, IntFlag, auto


class Color(Enum):
    RED = auto()
    GREEN = auto()


class Level(IntEnum):
    LOW = 1
    HIGH = 2


class Perm(IntFlag):
    READ = 1
    WRITE = 2


print(Color.RED.name, Color.RED.value)
print(Color.GREEN.name, Color.GREEN.value)
print(int(Level.HIGH))
combo = Perm.READ | Perm.WRITE
print(int(combo), combo == Perm(3))
print((combo & Perm.READ) == Perm.READ)
