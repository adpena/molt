"""Purpose: differential coverage for enum basics."""

from enum import Enum, IntEnum, IntFlag, StrEnum, auto


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


class AutoName(Enum):
    def _generate_next_value_(name, start, count, last_values):
        return name


class SqlType(AutoName):
    DATE = auto()
    TABLE = auto()
    assert DATE == "DATE"


class History(Enum):
    @staticmethod
    def _generate_next_value_(name, start, count, last_values):
        previous = last_values[-1] if last_values else start - 1
        last_values.append(1000)
        return previous + count + 1

    FIRST = 20
    SECOND = auto()
    THIRD = SECOND + 5
    FOURTH = auto()


class Access(IntFlag):
    READ = 4
    WRITE = auto()


class Dialect(StrEnum):
    SPARK = auto()


print(SqlType.DATE.value, SqlType.TABLE.value)
print([item.value for item in History], Access.WRITE.value, Dialect.SPARK.value)
