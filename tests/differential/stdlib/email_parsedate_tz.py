"""Purpose: differential coverage for email.utils parsedate_tz."""

from email.utils import parsedate_tz


def main():
    parsed = parsedate_tz("Thu, 02 Jan 2020 03:04:05 -0000")
    print("parsed", parsed)


if __name__ == "__main__":
    main()
