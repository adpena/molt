"""Purpose: differential coverage for importlib.resources reads in subdirs."""

from importlib import resources


def main():
    base = resources.files("importlib")
    sub = base.joinpath("resources")
    if sub.is_dir():
        items = [entry.name for entry in sub.iterdir()]
        print("subdir", len(items) >= 0)
    else:
        print("subdir", False)


if __name__ == "__main__":
    main()
