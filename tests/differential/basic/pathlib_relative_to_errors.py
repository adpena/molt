"""Purpose: differential coverage for pathlib relative_to errors."""

from pathlib import Path


def main():
    base = Path("/tmp")
    target = Path("/var/log")
    try:
        target.relative_to(base)
    except Exception as exc:
        print("error", type(exc).__name__)

    rel = Path("/tmp/app").relative_to(Path("/tmp"))
    print("rel", rel.as_posix())


if __name__ == "__main__":
    main()
