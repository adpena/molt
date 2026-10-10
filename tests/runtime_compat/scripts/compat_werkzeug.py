import werkzeug

print("werkzeug", werkzeug.__version__)
from werkzeug.datastructures import Headers

h = Headers()
h["Content-Type"] = "text/html"
h["X-Custom"] = "test"
print("content-type:", h["Content-Type"])
print("len:", len(h))
