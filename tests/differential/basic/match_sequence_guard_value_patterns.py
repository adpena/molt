"""Purpose: differential coverage for match sequence/guard/value patterns."""

TARGET = 2

value = [1, 2, 3]
match value:
    case [1, x, 3] if x == TARGET:
        print("seq_guard", x)
    case _:
        print("seq_guard", "miss")

match 5:
    case TARGET:
        print("value", "hit")
    case _:
        print("value", "miss")
