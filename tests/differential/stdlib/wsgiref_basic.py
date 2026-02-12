"""Purpose: differential coverage for wsgiref basic."""

from wsgiref.headers import Headers
from wsgiref.util import setup_testing_defaults


env: dict[str, str] = {}
setup_testing_defaults(env)
print(env["REQUEST_METHOD"], env["SERVER_NAME"])

headers = Headers([("Content-Type", "text/plain")])
headers.add_header("X-Test", "1")

print(str(headers).strip().splitlines())
