"""Purpose: differential coverage for pattern matching basic."""

value = {"type": "point", "x": 1, "y": 2}

match value:
    case {"type": "point", "x": x, "y": y}:
        print(x, y)
    case _:
        print("no")
