"""Purpose: differential coverage for email.parser basics."""

from email.parser import Parser


def main():
    source = "Subject: Hello\nFrom: a@example.com\n\nBody\n"
    msg = Parser().parsestr(source)
    print("subject", msg["Subject"])
    print("body", msg.get_payload().strip())


if __name__ == "__main__":
    main()
