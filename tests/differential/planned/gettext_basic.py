"""Purpose: differential coverage for gettext basics."""

import gettext

print(gettext.gettext("hello"))
print(gettext.ngettext("file", "files", 1))
print(gettext.ngettext("file", "files", 2))
