import jwt

print("pyjwt", jwt.__version__)
payload = {"user": "molt", "role": "compiler"}
token = jwt.encode(payload, "secret", algorithm="HS256")
print("token type:", type(token).__name__)
decoded = jwt.decode(token, "secret", algorithms=["HS256"])
print("decoded:", decoded)
print("roundtrip:", decoded == payload)
