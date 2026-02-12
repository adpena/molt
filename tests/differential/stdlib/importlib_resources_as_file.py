"""Purpose: differential coverage for importlib.resources.as_file."""

from importlib import resources


def main():
    resource = resources.files("importlib").joinpath("__init__.py")
    with resources.as_file(resource) as path:
        print("exists", path.exists())
        print("suffix", path.suffix)


if __name__ == "__main__":
    main()
