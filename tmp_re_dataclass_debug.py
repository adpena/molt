import re

print("field_names", getattr(re._Literal, "__molt_dataclass_field_names__", None))
print("flags", getattr(re._Literal, "__molt_dataclass_flags__", None))
node = re._Literal("foo")
print("node_class", node.__class__)
print("node_dict", getattr(node, "__dict__", None))
print("node_text", getattr(node, "text", None))
