import packaging
from packaging.version import Version
from packaging.requirements import Requirement

v = Version("3.12.1")
print("packaging", packaging.__version__)
print("version:", v)
print("major:", v.major)
print("minor:", v.minor)

req = Requirement("requests>=2.20")
print("req name:", req.name)
print("req specifier:", req.specifier)
