"""Purpose: differential coverage for email.message basics."""

from email import message_from_bytes, policy
from email.message import EmailMessage


def main():
    msg = EmailMessage()
    msg["Subject"] = "Hi"
    msg["From"] = "a@example.com"
    msg["To"] = "b@example.com"
    msg.set_content("Body")

    raw = msg.as_bytes()
    parsed = message_from_bytes(raw, policy=policy.default)
    print("subject", parsed["Subject"])
    print("body", parsed.get_body().get_content().strip())


if __name__ == "__main__":
    main()
