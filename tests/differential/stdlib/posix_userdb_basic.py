# MOLT_META: platforms=linux,macos,freebsd
# MOLT_ENV: MOLT_CAPABILITIES=env.read
"""Purpose: differential coverage for posix userdb basic."""

import grp
import pwd

print(bool(grp.getgrnam(grp.getgrgid(pwd.getpwnam(pwd.getpwuid(0).pw_name).pw_gid).gr_name).gr_name))
