"""Purpose: differential coverage for dotted name value patterns."""

class Box:
    VALUE = 7


value = 7
match value:
    case Box.VALUE:
        print("dotted", "hit")
    case _:
        print("dotted", "miss")

Box.VALUE = 8
match value:
    case Box.VALUE:
        print("dotted2", "hit")
    case _:
        print("dotted2", "miss")
