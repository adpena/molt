"""Purpose: differential coverage for tempfile basics."""

import os
import tempfile


def main():
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "sample.txt")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write("hi")
        print("exists", os.path.exists(path))

        with tempfile.NamedTemporaryFile(dir=tmp, delete=False) as handle:
            name = handle.name
            handle.write(b"x")
        print("named", os.path.exists(name))

    print("cleanup", not os.path.exists(tmp))


if __name__ == "__main__":
    main()
