"""Purpose: validate runtime-lowered importlib SourceFileLoader restricted execution."""

import importlib.util
import os
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    mod_path = os.path.join(tmp, "loader_exec_demo.py")
    with open(mod_path, "w", encoding="utf-8") as handle:
        handle.write(
            '"""module doc"""\n'
            "pass\n"
            "ival = 7\n"
            "fval = 1.25\n"
            "sval = 'hello\\nworld'\n"
            "bval = True\n"
            "nval = None\n"
        )

    spec = importlib.util.spec_from_file_location("loader_exec_demo", mod_path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules.pop("loader_exec_demo", None)
    spec.loader.exec_module(module)

    print(module.__doc__)
    print(module.ival, module.fval, module.bval, module.nval is None)
    print(module.sval == "hello\nworld")
