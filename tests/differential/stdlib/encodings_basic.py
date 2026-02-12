"""Purpose: differential coverage for encodings basic API surface."""

import encodings
import encodings.aliases

print(hasattr(encodings, "__path__"))
print("utf_8" in encodings.aliases.aliases)
