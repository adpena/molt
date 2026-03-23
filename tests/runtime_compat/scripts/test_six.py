import six

print("six", six.__version__)
print("PY2:", six.PY2)
print("PY3:", six.PY3)
print("text_type:", six.text_type.__name__)
print("integer_types:", [t.__name__ for t in six.integer_types])
