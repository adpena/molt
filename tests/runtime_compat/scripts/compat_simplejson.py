import simplejson

print("simplejson", simplejson.__version__)
data = {"name": "Molt", "values": [1, 2, 3]}
encoded = simplejson.dumps(data, sort_keys=True)
decoded = simplejson.loads(encoded)
print("encoded:", encoded)
print("equal:", data == decoded)
