# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources module object files."""

import importlib.resources as resources
import tests.differential.planned as pkg

root = resources.files(pkg)
print(root.is_dir())
