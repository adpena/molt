# MOLT_ENV: PYTHONPATH=src:tests/differential/basic
"""Purpose: a call to another module's function links to the symbol its
definer compiled, including a function named `main` and names whose
symbols collide."""

from cross_module_symbols_pkg import helpers

for _ in range(2):
    print(helpers.main())
print(helpers.molt_user_main())
print(helpers.double(21))
print(helpers.step(1), helpers.step_poll(1))
