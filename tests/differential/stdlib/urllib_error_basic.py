"""Purpose: differential coverage for urllib error basic."""

import urllib.error


print(issubclass(urllib.error.HTTPError, urllib.error.URLError))
print(urllib.error.ContentTooShortError.__base__ is urllib.error.URLError)
