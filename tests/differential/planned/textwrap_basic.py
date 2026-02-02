"""Purpose: differential coverage for textwrap basics."""

import textwrap

wrapper = textwrap.TextWrapper(width=5)
print(wrapper.wrap("hello world"))
print(textwrap.indent("line1\nline2", "> "))
