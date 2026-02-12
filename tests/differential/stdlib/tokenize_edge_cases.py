"""Purpose: differential coverage for tokenize edge cases."""

import io
import tokenize

source = """# comment
name = 1  # inline
"""
stream = io.BytesIO(source.encode("utf-8"))

tokens = list(tokenize.tokenize(stream.readline))
print(tokens[1].type)
print(tokens[1].string)
print(tokens[2].string)
print(tokens[3].string)
