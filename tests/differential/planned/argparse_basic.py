"""Purpose: differential coverage for argparse basics."""

import argparse


def main():
    parser = argparse.ArgumentParser(exit_on_error=False)
    parser.add_argument("--count", type=int, default=1)
    parser.add_argument("name")

    args = parser.parse_args(["alice"])
    print("defaults", args.name, args.count)

    args2 = parser.parse_args(["--count", "3", "bob"])
    print("parsed", args2.name, args2.count)

    try:
        parser.parse_args(["--count", "bad", "carol"])
    except Exception as exc:
        print("error", type(exc).__name__)


if __name__ == "__main__":
    main()
