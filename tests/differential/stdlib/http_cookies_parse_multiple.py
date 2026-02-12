"""Purpose: differential coverage for http.cookies parsing multiple cookies."""

from http import cookies

c = cookies.SimpleCookie()
c.load("a=1; b=2")
print(sorted(c.keys()))
