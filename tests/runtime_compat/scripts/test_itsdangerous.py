import itsdangerous

print("itsdangerous", itsdangerous.__version__)
s = itsdangerous.URLSafeSerializer("secret-key")
signed = s.dumps({"user": "molt"})
print("signed type:", type(signed).__name__)
loaded = s.loads(signed)
print("loaded:", loaded)
print("roundtrip:", loaded == {"user": "molt"})
