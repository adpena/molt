"""Purpose: differential coverage for re basic."""

import re


text = "Hello 123 world"
print(bool(re.search(r"\d+", text)))
print(re.sub(r"\d+", "X", text))
print(re.findall(r"[A-Za-z]+", text))
print(re.split(r"\s+", text))

m = re.match(r"(Hello)\s+(\d+)", text)
print(m.group(1), m.group(2))
