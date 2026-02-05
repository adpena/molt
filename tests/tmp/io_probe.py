import io

buf = io.StringIO()
print(buf)
print(type(buf))
print("is None", buf is None)
print("has read", hasattr(buf, "read"))
print("has readline", hasattr(buf, "readline"))
print("iterable", hasattr(buf, "__iter__"))
