"""Purpose: differential coverage for email header folding."""

from email.message import EmailMessage


def main():
    msg = EmailMessage()
    msg["Subject"] = "Hello" * 20
    msg["From"] = "a@example.com"
    msg["To"] = "b@example.com"
    msg.set_content("Body")

    raw = msg.as_string()
    folded = "\n " in raw or "\n\t" in raw
    print("folded", folded)


if __name__ == "__main__":
    main()
