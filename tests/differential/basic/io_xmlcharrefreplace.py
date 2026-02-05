"""Purpose: differential coverage for xmlcharrefreplace in text I/O."""

import io


def main() -> None:
    buf = io.BytesIO()
    wrapper = io.TextIOWrapper(
        buf,
        encoding="ascii",
        errors="xmlcharrefreplace",
        newline="",
    )
    wrapper.write("caf\u00e9 \U0001D11E")
    wrapper.flush()
    print(buf.getvalue())


if __name__ == "__main__":
    main()
