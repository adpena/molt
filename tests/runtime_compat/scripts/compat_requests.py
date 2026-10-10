import requests

print("requests", requests.__version__)
print("get exists:", hasattr(requests, "get"))
print("post exists:", hasattr(requests, "post"))
print("Session exists:", hasattr(requests, "Session"))
