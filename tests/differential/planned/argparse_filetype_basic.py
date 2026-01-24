"""Purpose: differential coverage for argparse FileType."""

import argparse
import tempfile


def main():
    with tempfile.NamedTemporaryFile("w+", delete=True) as handle:
        handle.write("hi")
        handle.flush()

        parser = argparse.ArgumentParser(exit_on_error=False)
        parser.add_argument("file", type=argparse.FileType("r"))
        args = parser.parse_args([handle.name])
        print("read", args.file.read().strip())
        args.file.close()


if __name__ == "__main__":
    main()
