"""Purpose: differential coverage for urllib.parse urljoin/urlparse."""

import urllib.parse

print(urllib.parse.urljoin("http://example.com/a/", "b"))
print(urllib.parse.urlparse("https://example.com/path?x=1"))
