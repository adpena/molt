"""Purpose: differential coverage for email policy/header handling."""

from email import policy
from email.message import EmailMessage


def main():
    msg = EmailMessage(policy=policy.default)
    msg["Subject"] = "Hello"
    msg["X-Test"] = "value"
    msg.set_content("Body")
    raw = msg.as_bytes()
    print("has_subject", b"Subject" in raw)
    print("has_x", b"X-Test" in raw)


if __name__ == "__main__":
    main()
