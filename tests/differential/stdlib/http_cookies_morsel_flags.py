"""Purpose: differential coverage for http.cookies Morsel flags."""

from http import cookies

c = cookies.SimpleCookie()
c["a"] = "b"
c["a"]["httponly"] = True
c["a"]["secure"] = True
print(c.output(sep="; "))
