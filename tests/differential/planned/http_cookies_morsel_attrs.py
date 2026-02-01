"""Purpose: differential coverage for http.cookies Morsel attributes."""

from http import cookies

c = cookies.SimpleCookie()
c["a"] = "b"
c["a"]["path"] = "/"
print(c["a"].OutputString())
