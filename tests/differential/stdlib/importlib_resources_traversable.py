"""Purpose: differential coverage for importlib.resources traversable APIs."""

from importlib import resources


def main():
    base = resources.files("importlib")
    init = base.joinpath("__init__.py")
    print("is_file", init.is_file())
    print("name", init.name)
    entries = [entry.name for entry in base.iterdir() if entry.name.startswith("__")]
    print("entries", sorted(entries)[:3])


if __name__ == "__main__":
    main()
