"""Purpose: differential token numbering and exact operator type coverage."""

import io
import token
import tokenize

source = "x = 1\n# comment\n"
stream = io.BytesIO(source.encode("utf-8"))
tokens = list(tokenize.tokenize(stream.readline))
for item in tokens:
    print(item.type, token.tok_name[item.type], repr(item.string), item.exact_type)

assert tokens[0].type == token.ENCODING
assert tokens[2].type == token.OP
assert tokens[2].exact_type == token.EQUAL

# Exercise exact_type independently of support for scanning compound operators.
for kind, text in (
    (token.OP, "**="),
    (token.OP, ":="),
    (token.OP, "?"),
    (token.NAME, "+"),
):
    item = tokenize.TokenInfo(kind, text, (1, 0), (1, len(text)), text)
    print("exact", kind, text, item.exact_type)
