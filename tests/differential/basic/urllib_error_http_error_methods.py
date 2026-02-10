"""Purpose: differential coverage for urllib.error.HTTPError delegation helpers."""

import urllib.error


class _DummyFP:
    def __init__(self) -> None:
        self.closed = False
        self._body = b"payload"
        self._headers = {"X-Test": "ok"}
        self.url = "http://example.test/fp"

    def read(self, _size: int = -1) -> bytes:
        return self._body

    def close(self) -> None:
        self.closed = True

    def info(self):
        return self._headers

    def geturl(self):
        return self.url


fp = _DummyFP()
err = urllib.error.HTTPError(
    "http://example.test/origin",
    404,
    "Not Found",
    {"X-Origin": "origin"},
    fp,
)

print(err.getcode(), err.geturl())
print(err.read().decode("ascii"))
print(err.info().get("X-Test"))
err.close()
print("closed", fp.closed)
