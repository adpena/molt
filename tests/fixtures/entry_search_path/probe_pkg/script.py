import sibling

print(sibling.VALUE)
print("before-missing-package")
import probe_pkg  # noqa: E402 - the sibling import must complete before failure.

print("unexpected-import", probe_pkg)
