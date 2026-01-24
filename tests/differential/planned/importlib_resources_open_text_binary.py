"""Purpose: differential coverage for importlib.resources open_text/open_binary."""

from importlib import resources


def main():
    with resources.open_text("importlib", "__init__.py") as handle:
        head = handle.read(50)
        print("text", "importlib" in head)

    with resources.open_binary("importlib", "__init__.py") as handle:
        data = handle.read(10)
        print("binary", len(data))


if __name__ == "__main__":
    main()
