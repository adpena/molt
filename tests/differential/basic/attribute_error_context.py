"""Purpose: differential coverage for AttributeError name/context."""


def main():
    class Demo:
        pass

    demo = Demo()
    try:
        demo.missing
    except AttributeError as exc:
        print("attr", exc.name, exc.obj is demo)


if __name__ == "__main__":
    main()
