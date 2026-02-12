"""Purpose: differential coverage for urllib.error intrinsic-backed classes."""

import urllib.error


print(issubclass(urllib.error.HTTPError, urllib.error.URLError))
print(urllib.error.ContentTooShortError.__base__ is urllib.error.URLError)

url_err = urllib.error.URLError("dns failed")
print(type(url_err).__name__, url_err.reason, str(url_err), url_err.args)

http_err = urllib.error.HTTPError(
    "https://example.com/missing",
    404,
    "Not Found",
    {"content-type": "text/plain"},
    None,
)
print(http_err.code, http_err.reason, http_err.filename, http_err.headers["content-type"])
print(str(http_err), http_err.args)

short_err = urllib.error.ContentTooShortError("short body", b"abc")
print(short_err.reason, short_err.content, short_err.args)
