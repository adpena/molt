"""Purpose: differential coverage for gettext basics."""

import gettext


marker = object()
print(gettext.gettext(marker) is marker)
print(gettext.gettext("hello"))
print(gettext.ngettext("file", "files", 1))
print(gettext.ngettext("file", "files", 2))
print(gettext.ngettext("file", "files", True))
