"""Purpose: differential coverage for email address parsing."""

from email.utils import getaddresses


def main():
    header = "Alice <a@example.com>, Bob <b@example.com>"
    addresses = getaddresses([header])
    print("count", len(addresses))
    print("first", addresses[0][0], addresses[0][1])


if __name__ == "__main__":
    main()
