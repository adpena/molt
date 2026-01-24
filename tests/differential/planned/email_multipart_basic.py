"""Purpose: differential coverage for email multipart messages."""

from email.message import EmailMessage


def main():
    msg = EmailMessage()
    msg["Subject"] = "Multipart"
    msg.set_content("plain text")
    msg.add_alternative("<b>html</b>", subtype="html")

    payload = msg.get_payload()
    print("is_multipart", msg.is_multipart())
    print("parts", len(payload))
    print("types", [part.get_content_type() for part in payload])


if __name__ == "__main__":
    main()
