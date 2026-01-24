"""Purpose: differential coverage for email.utils date helpers."""

from email.utils import format_datetime, parsedate_to_datetime
from datetime import datetime, timezone


def main():
    dt = datetime(2020, 1, 2, 3, 4, 5, tzinfo=timezone.utc)
    formatted = format_datetime(dt)
    parsed = parsedate_to_datetime(formatted)
    print("formatted", formatted)
    print("parsed", parsed == dt)


if __name__ == "__main__":
    main()
