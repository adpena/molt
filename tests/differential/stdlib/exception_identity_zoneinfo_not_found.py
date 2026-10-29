# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-207 expect_molt_returncode=0 expect_molt_stdout_sha256=3b6df66396209a3c0d8ab25fa9add132fb20b705e962a32f4248202a1763ffc9
"""Purpose: an unknown zone raises zoneinfo.ZoneInfoNotFoundError (a KeyError)."""

import zoneinfo


def main():
    try:
        zoneinfo.ZoneInfo("No/Such_Zone")
    except zoneinfo.ZoneInfoNotFoundError as exc:
        print(
            "ZoneInfoNotFoundError",
            type(exc) is zoneinfo.ZoneInfoNotFoundError,
            isinstance(exc, KeyError),
        )
    except KeyError as exc:
        print("KeyError only", type(exc).__name__)
    except Exception as exc:  # noqa: BLE001 - the observation is the escape
        print("escaped", type(exc).__name__)
    else:
        print("no error")
    try:
        zoneinfo.ZoneInfo("No/Such_Zone")
    except KeyError:
        print("caught as KeyError")
    except Exception as exc:  # noqa: BLE001
        print("escaped KeyError handler", type(exc).__name__)
    print("base", zoneinfo.ZoneInfoNotFoundError.__mro__[1].__name__)


if __name__ == "__main__":
    main()
