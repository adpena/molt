"""Purpose: differential coverage for unicode normalization."""

import unicodedata


s = "e\u0301"
print(unicodedata.normalize("NFC", s) == "\u00e9")
print(unicodedata.normalize("NFD", "\u00e9") == s)
