"""Purpose: verify __import__ fromlist import path stays wired through importlib."""


pkg = __import__("importlib", fromlist=["util"])

print(pkg.__name__)
print(hasattr(pkg, "util"))
print(pkg.util.__name__.split(".")[-1])
