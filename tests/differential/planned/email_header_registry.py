"""Purpose: differential coverage for email header registry."""

from email.headerregistry import Address
from email.message import EmailMessage


def main():
    msg = EmailMessage()
    msg["From"] = Address("Alice", "a", "example.com")
    msg["To"] = Address("Bob", "b", "example.com")
    msg["Subject"] = "Hi"
    msg.set_content("Body")

    print("from", msg["From"].addr_spec)
    print("to", msg["To"].addr_spec)


if __name__ == "__main__":
    main()
