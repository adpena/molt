"""Purpose: differential coverage for importlib.resources path/iterdir."""

from importlib import resources


def main():
    base = resources.files("importlib")
    entries = [entry for entry in base.iterdir() if entry.name.endswith(".py")]
    print("count", len(entries) > 0)


if __name__ == "__main__":
    main()
