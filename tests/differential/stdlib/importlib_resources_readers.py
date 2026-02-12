"""Purpose: differential coverage for importlib.resources basic reads."""

from importlib import resources


def main():
    text = resources.read_text("importlib", "__init__.py")
    print("has_importlib", "importlib" in text)
    data = resources.read_binary("importlib", "__init__.py")
    print("binary_len", len(data) > 0)


if __name__ == "__main__":
    main()
