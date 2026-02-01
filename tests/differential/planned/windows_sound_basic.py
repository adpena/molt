# MOLT_META: platforms=windows
# MOLT_ENV: MOLT_CAPABILITIES=env.read
"""Purpose: differential coverage for windows sound basic."""

import winsound

print(hasattr(winsound, 'Beep'))
