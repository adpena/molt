"""Purpose: differential coverage for email attachments."""

from email.message import EmailMessage


def main():
    msg = EmailMessage()
    msg["Subject"] = "Attachment"
    msg.set_content("Body")
    msg.add_attachment(b"data", maintype="application", subtype="octet-stream", filename="file.bin")

    print("is_multipart", msg.is_multipart())
    parts = msg.get_payload()
    print("parts", len(parts))
    attachment = parts[1]
    print("filename", attachment.get_filename())
    print("ctype", attachment.get_content_type())


if __name__ == "__main__":
    main()
