"""Purpose: differential coverage for email.utils make_msgid."""

from email.utils import make_msgid


def main():
    msgid = make_msgid()
    print("msgid", msgid.startswith("<") and msgid.endswith(">"))
    msgid2 = make_msgid("example.com")
    print("domain", "@example.com" in msgid2)


if __name__ == "__main__":
    main()
