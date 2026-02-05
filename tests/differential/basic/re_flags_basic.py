"""Purpose: differential coverage for re flags + non-greedy quantifiers."""

import re


def show(label, value):
    print(label, value)


m = re.search("FOO", "foo", re.IGNORECASE)
show("ignorecase", m.group(0) if m else None)

m = re.search("a.b", "a\nb")
show("dotall_default", m.group(0) if m else None)

m = re.search("a.b", "a\nb", re.DOTALL)
show("dotall", m.group(0) if m else None)

m = re.search(r"^b", "a\nb")
show("multiline_default", m.group(0) if m else None)

m = re.search(r"^b", "a\nb", re.MULTILINE)
show("multiline", m.group(0) if m else None)

m = re.search(r"a+?", "aaaa")
show("nongreedy", m.group(0) if m else None)
