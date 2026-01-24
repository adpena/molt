"""Purpose: differential coverage for dotted value patterns with shadowing."""

class A:
    VALUE = 1

class B:
    VALUE = 2


def run(value):
    Box = B
    match value:
        case Box.VALUE:
            print("local", "hit")
        case _:
            print("local", "miss")

    match value:
        case A.VALUE:
            print("global", "hit")
        case _:
            print("global", "miss")


run(2)
run(1)
