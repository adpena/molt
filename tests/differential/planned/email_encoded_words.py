"""Purpose: differential coverage for encoded-words in headers."""

from email.header import Header
from email.message import EmailMessage


def main():
    msg = EmailMessage()
    msg["Subject"] = Header("café", "utf-8")
    msg.set_content("Body")
    raw = msg.as_bytes()
    print("encoded", b"=?utf-8?" in raw.lower())


if __name__ == "__main__":
    main()
