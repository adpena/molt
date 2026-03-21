"""Record of phased-in incompatible language changes.

Each line is of the form:

    FeatureName = "_Feature(" OptionalRelease "," MandatoryRelease ","
                              CompilerFlag ")"

where, normally, OptionalRelease < MandatoryRelease, and both are 5-tuples
of the same form as sys.version_info:

    (PY_MAJOR_VERSION, # the 2 in 2.1.0a3; an int
     PY_MINOR_VERSION, # the 1; an int
     PY_MICRO_VERSION, # the 0; an int
     PY_RELEASE_LEVEL, # "alpha", "beta", "candidate" or "final"; string
     PY_RELEASE_SERIAL # the 3; an int
    )

OptionalRelease records the first release in which

    from __future__ import FeatureName

was accepted.

In the case of MandatoryReleases that have not yet occurred,
MandatoryRelease predicts the release in which the feature will become part
of the language.

Else MandatoryRelease records when the feature became part of the language;
in releases at or after that, modules no longer need

    from __future__ import FeatureName

to use the feature in question, but may continue to use such imports.

MandatoryRelease may also be None, meaning that a planned feature got
dropped or that the release version is undetermined.

Instances of class _Feature have two corresponding methods,
.getOptionalRelease() and .getMandatoryRelease().

CompilerFlag is the (bitfield) flag that should be passed in the fourth
argument to the builtin function compile() to enable the feature in
dynamically compiled code. This flag is stored in the .compiler_flag
attribute on _Feature instances. These values must match the appropriate
#defines of CO_xxx flags in Include/cpython/compile.h.

No feature line is ever to be deleted from this file.
"""

from _intrinsics import require_intrinsic as _require_intrinsic

_FALLBACK_FEATURE_ROWS = (
    ("nested_scopes", (2, 1, 0, "beta", 1), (2, 2, 0, "alpha", 0), 16),
    ("generators", (2, 2, 0, "alpha", 1), (2, 3, 0, "final", 0), 0),
    ("division", (2, 2, 0, "alpha", 2), (3, 0, 0, "alpha", 0), 131072),
    (
        "absolute_import",
        (2, 5, 0, "alpha", 1),
        (3, 0, 0, "alpha", 0),
        262144,
    ),
    ("with_statement", (2, 5, 0, "alpha", 1), (2, 6, 0, "alpha", 0), 524288),
    ("print_function", (2, 6, 0, "alpha", 2), (3, 0, 0, "alpha", 0), 1048576),
    ("unicode_literals", (2, 6, 0, "alpha", 2), (3, 0, 0, "alpha", 0), 2097152),
    ("barry_as_FLUFL", (3, 1, 0, "alpha", 2), (4, 0, 0, "alpha", 0), 4194304),
    ("generator_stop", (3, 5, 0, "beta", 1), (3, 7, 0, "alpha", 0), 8388608),
    ("annotations", (3, 7, 0, "beta", 1), None, 16777216),
)


def _load_feature_rows():
    try:
        future_features = _require_intrinsic("molt_future_features")
    except Exception:
        return list(_FALLBACK_FEATURE_ROWS)
    try:
        result = future_features()
        if result is None:
            return list(_FALLBACK_FEATURE_ROWS)
        rows = list(result)
        if not rows:
            return list(_FALLBACK_FEATURE_ROWS)
        return rows
    except Exception:
        return list(_FALLBACK_FEATURE_ROWS)


_feature_rows = _load_feature_rows()
all_feature_names = [str(row[0]) for row in _feature_rows]

__all__ = ["all_feature_names"] + all_feature_names

# The CO_xxx symbols are defined here under the same names defined in
# code.h and used by compile.h, so that an editor search will find them here.
# However, they're not exported in __all__, because they don't really belong to
# this module.
_feature_flags = {str(name): int(flag) for name, _, _, flag in _feature_rows}
CO_NESTED = _feature_flags["nested_scopes"]  # nested_scopes
CO_GENERATOR_ALLOWED = _feature_flags["generators"]  # generators (obsolete)
CO_FUTURE_DIVISION = _feature_flags["division"]  # division
CO_FUTURE_ABSOLUTE_IMPORT = _feature_flags["absolute_import"]
CO_FUTURE_WITH_STATEMENT = _feature_flags["with_statement"]
CO_FUTURE_PRINT_FUNCTION = _feature_flags["print_function"]
CO_FUTURE_UNICODE_LITERALS = _feature_flags["unicode_literals"]
CO_FUTURE_BARRY_AS_BDFL = _feature_flags["barry_as_FLUFL"]
CO_FUTURE_GENERATOR_STOP = _feature_flags["generator_stop"]
CO_FUTURE_ANNOTATIONS = _feature_flags["annotations"]


class _Feature:
    def __init__(self, optionalRelease, mandatoryRelease, compiler_flag):
        self.optional = optionalRelease
        self.mandatory = mandatoryRelease
        self.compiler_flag = compiler_flag

    def getOptionalRelease(self):
        """Return first release in which this feature was recognized.

        This is a 5-tuple, of the same form as sys.version_info.
        """
        return self.optional

    def getMandatoryRelease(self):
        """Return release in which this feature will become mandatory.

        This is a 5-tuple, of the same form as sys.version_info, or, if
        the feature was dropped, or the release date is undetermined, is None.
        """
        return self.mandatory

    def __repr__(self):
        return "_Feature" + repr((self.optional, self.mandatory, self.compiler_flag))


for _name, _optional, _mandatory, _compiler_flag in _feature_rows:
    globals()[str(_name)] = _Feature(_optional, _mandatory, int(_compiler_flag))

del _feature_flags
del _feature_rows
del _load_feature_rows
