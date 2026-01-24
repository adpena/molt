"""Purpose: differential coverage for re.groupdict defaults."""

import re


def main():
    match = re.match(r"(?P<a>\d+)?-(?P<b>\w+)", "-abc")
    print("groupdict", match.groupdict())
    print("groupdict_default", match.groupdict(default="X"))


if __name__ == "__main__":
    main()
