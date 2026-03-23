import certifi

print("certifi", certifi.__version__)
path = certifi.where()
print("ca-bundle exists:", len(path) > 0)
print("path ends with .pem:", path.endswith(".pem"))
