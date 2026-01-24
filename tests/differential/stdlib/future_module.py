"""Purpose: differential coverage for future module."""

import __future__

print(__future__.all_feature_names)
print(isinstance(__future__.division, __future__._Feature))
print(__future__.division.getOptionalRelease())
print(__future__.division.getMandatoryRelease())
print(__future__.division.compiler_flag)
print(__future__.annotations.getMandatoryRelease())
