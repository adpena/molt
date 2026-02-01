"""Purpose: differential coverage for tokenize basic API surface."""

import io
import tokenize

source = "x = 1
"
stream = io.BytesIO(source.encode("utf-8"))

tokens = list(tokenize.tokenize(stream.readline))
print(tokens[1].string)
