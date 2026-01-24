"""Purpose: differential coverage for locals() mutation semantics."""


def main():
    def inner():
        x = 1
        locals()["x"] = 2
        locals()["y"] = 3
        return x, locals().get("y")

    print("inner", inner())


if __name__ == "__main__":
    main()
