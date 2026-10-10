import chardet

print("chardet", chardet.__version__)
result = chardet.detect(b"Hello, world!")
print("encoding:", result["encoding"])
print("confidence >= 0.5:", result["confidence"] >= 0.5)
