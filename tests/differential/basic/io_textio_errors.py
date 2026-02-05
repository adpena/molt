"""Purpose: validate TextIOWrapper error handling."""

import io

raw = io.BytesIO(b"\xff")
text = io.TextIOWrapper(raw, encoding="utf-8", errors="replace")
print(text.read())
