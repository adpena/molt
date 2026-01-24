"""Purpose: differential coverage for http cookies basic."""

from http.cookies import SimpleCookie


cookies = SimpleCookie()
cookies["session"] = "abc"
cookies["session"]["path"] = "/"
cookies["session"]["httponly"] = True
cookies["theme"] = "light"

rows = []
for key, morsel in sorted(cookies.items()):
    rows.append(
        (
            key,
            morsel.value,
            morsel["path"],
            bool(morsel["httponly"]),
        )
    )

print(rows)
