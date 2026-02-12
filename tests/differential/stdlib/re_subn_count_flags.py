"""Purpose: differential coverage for re.subn count + flags."""

import re


def main():
    source = "AaA"
    print("subn", re.subn("a", "b", source, flags=re.I))
    print("subn_count", re.subn("a", "b", source, count=1, flags=re.I))


if __name__ == "__main__":
    main()
