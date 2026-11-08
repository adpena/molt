# MOLT_ENV: TZ=America/St_Johns
# MOLT_META: backends=jit,llvm,native
# WASM hosts report CLDR zone names (GMT+5:30) until the runtime owns IANA
# zone data: docs/design/wasm_hosts.md, section 2.
"""Purpose: time zone constants and localtime under TZ=America/St_Johns.

A half-hour offset with daylight saving.
"""

import time

print(time.timezone, time.altzone, time.daylight, time.tzname)
for instant in (1767268800, 1783080000):
    local = time.localtime(instant)
    print(local.tm_hour, local.tm_isdst, local.tm_zone, local.tm_gmtoff)
    print(time.mktime(local) == instant)
