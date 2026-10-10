"""Purpose: validate TextIOWrapper error handling."""

import io

raw = io.BytesIO(b"\xff")
text = io.TextIOWrapper(raw, encoding="utf-8", errors="replace")
print(text.read())


# Encoding sees the newline-translated Python string. The exception object and
# span must describe that string, with identity retained when bytes are unchanged.
for newline, value in (("\r\n", "a\né€z"), ("\r", "a\né€z"),
                       ("\n", "a\né€z"), ("\r\n", "aé€z")):
    raw = io.BytesIO()
    text = io.TextIOWrapper(raw, encoding="ascii", errors="strict", newline=newline)
    try:
        text.write(value)
    except UnicodeEncodeError as exc:
        text.flush()
        print("text-encode-error", repr(newline), exc.encoding, exc.start, exc.end,
              repr(exc.object), exc.object is value, exc.reason, raw.getvalue())
    text.close()

raw = io.BytesIO()
text = io.TextIOWrapper(raw, encoding="ascii", errors="missing-encode-handler",
                        newline="\r\n")
print("text-unused-handler", text.write("a\nz"))
text.flush()
print(raw.getvalue())
text.close()
