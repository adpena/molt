"""Purpose: differential coverage for email.utils make_msgid edge case."""

from email.utils import make_msgid


def main():
    msgid = make_msgid("example.com")
    print("has_domain", msgid.endswith("@example.com>") or "@example.com>" in msgid)


if __name__ == "__main__":
    main()
