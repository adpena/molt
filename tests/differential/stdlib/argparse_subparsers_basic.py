"""Purpose: differential coverage for argparse subparsers."""

import argparse


def main():
    parser = argparse.ArgumentParser(exit_on_error=False)
    subparsers = parser.add_subparsers(dest="cmd", required=True)

    run = subparsers.add_parser("run")
    run.add_argument("name")

    info = subparsers.add_parser("info")
    info.add_argument("--verbose", action="store_true")

    args = parser.parse_args(["run", "demo"])
    print("cmd", args.cmd, args.name)

    args2 = parser.parse_args(["info", "--verbose"])
    print("info", args2.cmd, args2.verbose)

    try:
        parser.parse_args(["unknown"])  # invalid subcommand
    except Exception as exc:
        print("error", type(exc).__name__)


if __name__ == "__main__":
    main()
