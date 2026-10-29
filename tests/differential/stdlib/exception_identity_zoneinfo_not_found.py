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
