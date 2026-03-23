import invoke

print("invoke", invoke.__version__)
print("task exists:", hasattr(invoke, "task"))
print("Context exists:", hasattr(invoke, "Context"))
print("Collection exists:", hasattr(invoke, "Collection"))
