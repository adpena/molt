"""Purpose: differential coverage for string format method."""


def main() -> None:
    class Obj:
        def __init__(self) -> None:
            self.x = "X"

        def __str__(self) -> str:
            return "OBJ"

    obj = Obj()
    data = ["a", "b"]
    fmt = "{0} {1!r} {name} {0.x} {1[1]} {name:{width}} {{ok}}"
    print(fmt.format(obj, data, name="hi", width=5))


if __name__ == "__main__":
    main()
