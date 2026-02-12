"""Purpose: differential coverage for http.cookies basics."""

from http import cookies

c = cookies.SimpleCookie()
c["a"] = "b"
print(c.output().strip())
