"""Purpose: differential coverage for http.cookies output formatting."""

from http import cookies

c = cookies.SimpleCookie()
c["a"] = "b"
print(c.output(sep="; "))
