"""Purpose: differential coverage for shlex basics."""

import shlex

print(shlex.split("a b 'c d'"))
lexer = shlex.shlex("one two", posix=True)
lexer.whitespace_split = True
print(list(lexer))
