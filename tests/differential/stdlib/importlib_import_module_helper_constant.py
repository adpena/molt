"""Purpose: ensure helper-call constant module names are discovered for import graph closure."""

import importlib

TOP_LEVEL_MODULE = "errno"
SUBMODULE = "importlib.util"


def load_module(name: str):
    return importlib.import_module(name)


top = load_module(TOP_LEVEL_MODULE)
sub = load_module(SUBMODULE)

print(top.__name__)
print(sub.__name__)
print("EINTR" in top.__dict__)
