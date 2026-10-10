"""`json.__main__` compatibility shim.

In CPython, `json.__main__` exists starting in 3.14.
Version-gated absence for earlier versions is handled at importlib boundary.
"""


if __name__ == "__main__":
    import json.tool as _tool

    _tool.main()
