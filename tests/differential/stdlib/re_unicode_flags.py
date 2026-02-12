"""Purpose: differential coverage for re unicode flags."""

import re


text = "\u0391\u0392\u0393abc"  # Greek/Latin mix
pattern = re.compile(r"[A-Za-z]+", re.IGNORECASE)
print(pattern.findall(text))

pattern_u = re.compile(r"\w+", re.UNICODE)
print(pattern_u.findall(text))
