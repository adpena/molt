"""Purpose: differential coverage for urllib.parse urlsplit/urlunsplit."""

import urllib.parse

parts = urllib.parse.urlsplit("https://example.com/path?x=1#frag")
print(parts.scheme)
print(urllib.parse.urlunsplit(parts))
