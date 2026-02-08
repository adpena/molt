from __future__ import annotations

import shlex


print("split_basic", shlex.split("a b 'c d'"))
print("split_comments_off", shlex.split("a #b", comments=False))
print("split_comments_on", shlex.split("a #b", comments=True))
print("quote", shlex.quote("a b'c"))
print("join", shlex.join(["python3", "-c", "print('ok')"]))

lexer = shlex.shlex("one two", posix=True)
lexer.whitespace_split = True
print("lexer_whitespace_split", list(lexer))

lexer2 = shlex.shlex("a #b", posix=True)
lexer2.whitespace_split = True
print("lexer_comments_default", list(lexer2))

lexer3 = shlex.shlex("a #b", posix=True)
lexer3.whitespace_split = True
lexer3.commenters = ""
print("lexer_comments_off", list(lexer3))

lexer4 = shlex.shlex("a&&b", posix=True, punctuation_chars=True)
lexer4.whitespace_split = True
print("lexer_punct", list(lexer4))
