"""Purpose: differential coverage for re ascii locale flags."""

import re


text = "A_"
print(re.findall(r"\w+", text))
print(re.findall(r"\w+", text, re.ASCII))

try:
    re.compile(r"\w+", re.LOCALE)
    print("locale_ok")
except Exception as exc:
    print(type(exc).__name__)
