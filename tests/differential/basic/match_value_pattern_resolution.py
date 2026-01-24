"""Purpose: differential coverage for value pattern resolution and capture shadowing."""

TARGET = 3

value = 3
match value:
    case TARGET:
        print("value", "hit")
    case _:
        print("value", "miss")

match value:
    case OTHER:
        print("capture", OTHER)

print("target", TARGET)
