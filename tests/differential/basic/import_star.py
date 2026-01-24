"""Purpose: differential coverage for import star."""
# ruff: noqa: E402, F403, F405

import os
import sys

sys.path.insert(0, os.path.dirname(__file__))

from import_star_mod import *

print(f"mod_alpha={alpha}")
print(f"mod_hidden={_hidden}")
print(f"mod_beta={beta}")
print(f"mod_gamma={globals().get('gamma')}")

from import_star_plain import *

print(f"plain_alpha={alpha}")
print(f"plain_gamma={gamma}")
print(f"plain_hidden={globals().get('_beta')}")
