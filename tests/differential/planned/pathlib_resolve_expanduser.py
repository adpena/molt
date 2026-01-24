"""Purpose: differential coverage for Path.expanduser/resolve."""

from pathlib import Path
import tempfile


def main():
    home = Path("~").expanduser()
    print("home_abs", home.is_absolute())

    with tempfile.TemporaryDirectory() as tmp:
        base = Path(tmp)
        target = base / "a" / "b"
        target.mkdir(parents=True)
        probe = base / "a" / "." / "b"
        print("resolve", probe.resolve() == target.resolve())


if __name__ == "__main__":
    main()
