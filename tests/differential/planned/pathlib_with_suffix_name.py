"""Purpose: differential coverage for pathlib with_name/with_suffix."""

from pathlib import Path


def main():
    path = Path("/tmp/demo.tar.gz")
    print("suffixes", path.suffixes)
    print("with_suffix", path.with_suffix(".zip").name)
    print("with_name", path.with_name("other.txt").name)


if __name__ == "__main__":
    main()
