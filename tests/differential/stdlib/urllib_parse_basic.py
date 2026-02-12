"""Purpose: differential coverage for urllib.parse basics."""

from urllib import parse

url = "https://example.com/path?x=1&y=2"
parts = parse.urlparse(url)
print(parts.scheme, parts.netloc, parts.path)
print(parse.urlunparse(parts))
print(parse.urlencode([("a", 1), ("b", "two")]))
print(parse.quote("a b"))
print(parse.unquote("a%20b"))
