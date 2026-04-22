import pygments
from pygments.lexers import PythonLexer

print("pygments", pygments.__version__)
lexer = PythonLexer()
tokens = list(lexer.get_tokens("x = 1"))
print("token count:", len(tokens))
print("first token type:", str(tokens[0][0]))
