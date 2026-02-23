"""Purpose: differential coverage for http.cookies Morsel attributes."""

from http import cookies

c = cookies.SimpleCookie()
c["a"] = "b"
c["a"]["path"] = "/"
c["a"]["secure"] = True
c["a"]["httponly"] = True
c["a"]["max-age"] = 60
c["a"]["expires"] = "Wed, 21 Oct 2015 07:28:00 GMT"

m = c["a"]
print((m["path"], bool(m["secure"]), bool(m["httponly"]), m["max-age"], m["expires"]))
print(m.OutputString())
