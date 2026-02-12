"""Purpose: differential coverage for datetime strptime locale edge."""

import datetime


text = "01/02/2024 03:04:05"
parsed = datetime.datetime.strptime(text, "%m/%d/%Y %H:%M:%S")
print(parsed.isoformat())

text2 = "2024-001"
parsed2 = datetime.datetime.strptime(text2, "%Y-%j")
print(parsed2.date().isoformat())
