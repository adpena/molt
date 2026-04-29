"""Strptime-related classes and functions.

Direct port of CPython 3.12's _strptime module — pure-Python; depends on
time, locale, calendar (all intrinsic-backed in molt), re, datetime, and
_thread. Used by `time.strptime` and `datetime.datetime.strptime`.

CLASSES:
    LocaleTime — Discovers and stores locale-specific time information.
    TimeRE     — Creates regexes for matching time-formatted text.

FUNCTIONS:
    _getlang  — Identify the language for the LC_TIME locale.
    _strptime — Parse a string into time components.
"""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY


import time
import locale
import calendar
from re import compile as re_compile
from re import sub as re_sub
from re import IGNORECASE
from re import escape as re_escape
from datetime import (
    date as datetime_date,
    timedelta as datetime_timedelta,
    timezone as datetime_timezone,
)
from _thread import allocate_lock as _thread_allocate_lock

__all__ = []


def _getlang():
    return locale.getlocale(locale.LC_TIME)


def _findall(haystack, needle):
    if not needle:
        return
    i = 0
    while True:
        i = haystack.find(needle, i)
        if i < 0:
            break
        yield i
        i += len(needle)


class LocaleTime(object):
    """Stores and handles locale-specific time information."""

    def __init__(self):
        self.lang = _getlang()
        self.__calc_weekday()
        self.__calc_month()
        self.__calc_am_pm()
        self.__calc_timezone()
        self.__calc_date_time()
        if _getlang() != self.lang:
            raise ValueError("locale changed during initialization")
        if time.tzname != self.tzname or time.daylight != self.daylight:
            raise ValueError("timezone changed during initialization")

    def __calc_weekday(self):
        a_weekday = [calendar.day_abbr[i].lower() for i in range(7)]
        f_weekday = [calendar.day_name[i].lower() for i in range(7)]
        self.a_weekday = a_weekday
        self.f_weekday = f_weekday

    def __calc_month(self):
        a_month = [calendar.month_abbr[i].lower() for i in range(13)]
        f_month = [calendar.month_name[i].lower() for i in range(13)]
        self.a_month = a_month
        self.f_month = f_month

    def __calc_am_pm(self):
        am_pm = []
        for hour in (1, 22):
            time_tuple = time.struct_time((1999, 3, 17, hour, 44, 55, 2, 76, 0))
            am_pm.append(time.strftime("%p", time_tuple).lower().strip())
        self.am_pm = am_pm

    def __calc_date_time(self):
        time_tuple = time.struct_time((1999, 3, 17, 22, 44, 55, 2, 76, 0))
        time_tuple2 = time.struct_time((1999, 1, 3, 1, 1, 1, 6, 3, 0))
        replacement_pairs = [
            ("1999", "%Y"),
            ("99", "%y"),
            ("22", "%H"),
            ("44", "%M"),
            ("55", "%S"),
            ("76", "%j"),
            ("17", "%d"),
            ("03", "%m"),
            ("3", "%m"),
            ("2", "%w"),
            ("10", "%I"),
        ]
        date_time = []
        for directive in ("%c", "%x", "%X"):
            current_format = time.strftime(directive, time_tuple).lower()
            current_format = current_format.replace("%", "%%")
            lst, fmt = self.__find_weekday_format(directive)
            if lst:
                current_format = current_format.replace(lst[2], fmt, 1)
            lst, fmt = self.__find_month_format(directive)
            if lst:
                current_format = current_format.replace(lst[3], fmt, 1)
            if self.am_pm[1]:
                current_format = current_format.replace(self.am_pm[1], "%p")
            for tz_values in self.timezone:
                for tz in tz_values:
                    if tz:
                        current_format = current_format.replace(tz, "%Z")
            for old, new in replacement_pairs:
                current_format = current_format.replace(old, new)
            if "00" in time.strftime(directive, time_tuple2):
                U_W = "%W"
            else:
                U_W = "%U"
            current_format = current_format.replace("11", U_W)
            date_time.append(current_format)
        self.LC_date_time = date_time[0]
        self.LC_date = date_time[1]
        self.LC_time = date_time[2]

    def __find_month_format(self, directive):
        full_indices = abbr_indices = None
        for m in range(1, 13):
            time_tuple = time.struct_time((1999, m, 17, 22, 44, 55, 2, 76, 0))
            datetime_str = time.strftime(directive, time_tuple).lower()
            indices = set(_findall(datetime_str, self.f_month[m]))
            if full_indices is None:
                full_indices = indices
            else:
                full_indices &= indices
            indices = set(_findall(datetime_str, self.a_month[m]))
            if abbr_indices is None:
                abbr_indices = indices
            else:
                abbr_indices &= indices
            if not full_indices and not abbr_indices:
                return None, None
        if full_indices:
            return self.f_month, "%B"
        if abbr_indices:
            return self.a_month, "%b"
        return None, None

    def __find_weekday_format(self, directive):
        full_indices = abbr_indices = None
        for wd in range(7):
            time_tuple = time.struct_time((1999, 3, 17, 22, 44, 55, wd, 76, 0))
            datetime_str = time.strftime(directive, time_tuple).lower()
            indices = set(_findall(datetime_str, self.f_weekday[wd]))
            if full_indices is None:
                full_indices = indices
            else:
                full_indices &= indices
            if self.f_weekday[wd] != self.a_weekday[wd]:
                indices = set(_findall(datetime_str, self.a_weekday[wd]))
            if abbr_indices is None:
                abbr_indices = indices
            else:
                abbr_indices &= indices
            if not full_indices and not abbr_indices:
                return None, None
        if full_indices:
            return self.f_weekday, "%A"
        if abbr_indices:
            return self.a_weekday, "%a"
        return None, None

    def __calc_timezone(self):
        try:
            time.tzset()
        except AttributeError:
            pass
        self.tzname = time.tzname
        self.daylight = time.daylight
        no_saving = frozenset({"utc", "gmt", self.tzname[0].lower()})
        if self.daylight:
            has_saving = frozenset({self.tzname[1].lower()})
        else:
            has_saving = frozenset()
        self.timezone = (no_saving, has_saving)


class TimeRE(dict):
    """Handle conversion from format directives to regexes."""

    def __init__(self, locale_time=None):
        if locale_time:
            self.locale_time = locale_time
        else:
            self.locale_time = LocaleTime()
        base = super()
        mapping = {
            "d": r"(?P<d>3[0-1]|[1-2]\d|0[1-9]|[1-9]| [1-9])",
            "f": r"(?P<f>[0-9]{1,6})",
            "H": r"(?P<H>2[0-3]|[0-1]\d|\d)",
            "I": r"(?P<I>1[0-2]|0[1-9]|[1-9]| [1-9])",
            "G": r"(?P<G>\d\d\d\d)",
            "j": r"(?P<j>36[0-6]|3[0-5]\d|[1-2]\d\d|0[1-9]\d|00[1-9]|[1-9]\d|0[1-9]|[1-9])",
            "m": r"(?P<m>1[0-2]|0[1-9]|[1-9])",
            "M": r"(?P<M>[0-5]\d|\d)",
            "S": r"(?P<S>6[0-1]|[0-5]\d|\d)",
            "U": r"(?P<U>5[0-3]|[0-4]\d|\d)",
            "w": r"(?P<w>[0-6])",
            "u": r"(?P<u>[1-7])",
            "V": r"(?P<V>5[0-3]|0[1-9]|[1-4]\d|\d)",
            "y": r"(?P<y>\d\d)",
            "Y": r"(?P<Y>\d\d\d\d)",
            "z": r"(?P<z>[+-]\d\d:?[0-5]\d(:?[0-5]\d(\.\d{1,6})?)?|(?-i:Z))",
            "A": self.__seqToRE(self.locale_time.f_weekday, "A"),
            "a": self.__seqToRE(self.locale_time.a_weekday, "a"),
            "B": self.__seqToRE(self.locale_time.f_month[1:], "B"),
            "b": self.__seqToRE(self.locale_time.a_month[1:], "b"),
            "p": self.__seqToRE(self.locale_time.am_pm, "p"),
            "Z": self.__seqToRE(
                (tz for tz_names in self.locale_time.timezone for tz in tz_names),
                "Z",
            ),
            "%": "%",
        }
        for d in "dmyHIMS":
            mapping["O" + d] = r"(?P<%s>\d\d|\d| \d)" % d
        mapping["Ow"] = r"(?P<w>\d)"
        mapping["W"] = mapping["U"].replace("U", "W")
        base.__init__(mapping)
        base.__setitem__("X", self.pattern(self.locale_time.LC_time))
        base.__setitem__("x", self.pattern(self.locale_time.LC_date))
        base.__setitem__("c", self.pattern(self.locale_time.LC_date_time))

    def __seqToRE(self, to_convert, directive):
        to_convert = sorted(to_convert, key=len, reverse=True)
        for value in to_convert:
            if value != "":
                break
        else:
            return ""
        regex = "|".join(re_escape(stuff) for stuff in to_convert)
        regex = "(?P<%s>%s" % (directive, regex)
        return "%s)" % regex

    def pattern(self, format):
        format = re_sub(r"([\\.^$*+?\(\){}\[\]|])", r"\\\1", format)
        format = re_sub(r"\s+", r"\\s+", format)
        format = re_sub(r"'", "['ʼ]", format)

        def repl(m):
            return self[m[1]]

        format = re_sub(r"%(O?.)", repl, format)
        return format

    def compile(self, format):
        return re_compile(self.pattern(format), IGNORECASE)


_cache_lock = _thread_allocate_lock()
_TimeRE_cache = TimeRE()
_CACHE_MAX_SIZE = 5
_regex_cache = {}


def _calc_julian_from_U_or_W(year, week_of_year, day_of_week, week_starts_Mon):
    first_weekday = datetime_date(year, 1, 1).weekday()
    if not week_starts_Mon:
        first_weekday = (first_weekday + 1) % 7
        day_of_week = (day_of_week + 1) % 7
    week_0_length = (7 - first_weekday) % 7
    if week_of_year == 0:
        return 1 + day_of_week - first_weekday
    else:
        days_to_week = week_0_length + (7 * (week_of_year - 1))
        return 1 + days_to_week + day_of_week


def _strptime(data_string, format="%a %b %d %H:%M:%S %Y"):
    for index, arg in enumerate([data_string, format]):
        if not isinstance(arg, str):
            msg = "strptime() argument {} must be str, not {}"
            raise TypeError(msg.format(index, type(arg)))

    global _TimeRE_cache, _regex_cache
    with _cache_lock:
        locale_time = _TimeRE_cache.locale_time
        if (
            _getlang() != locale_time.lang
            or time.tzname != locale_time.tzname
            or time.daylight != locale_time.daylight
        ):
            _TimeRE_cache = TimeRE()
            _regex_cache.clear()
            locale_time = _TimeRE_cache.locale_time
        if len(_regex_cache) > _CACHE_MAX_SIZE:
            _regex_cache.clear()
        format_regex = _regex_cache.get(format)
        if not format_regex:
            try:
                format_regex = _TimeRE_cache.compile(format)
            except KeyError as err:
                bad_directive = err.args[0]
                if bad_directive == "\\":
                    bad_directive = "%"
                del err
                raise ValueError(
                    "'%s' is a bad directive in format '%s'" % (bad_directive, format)
                ) from None
            except IndexError:
                raise ValueError("stray %% in format '%s'" % format) from None
            _regex_cache[format] = format_regex
    found = format_regex.match(data_string)
    if not found:
        raise ValueError(
            "time data %r does not match format %r" % (data_string, format)
        )
    if len(data_string) != found.end():
        raise ValueError("unconverted data remains: %s" % data_string[found.end() :])

    iso_year = year = None
    month = day = 1
    hour = minute = second = fraction = 0
    tz = -1
    gmtoff = None
    gmtoff_fraction = 0
    iso_week = week_of_year = None
    week_of_year_start = None
    weekday = julian = None
    found_dict = found.groupdict()
    for group_key in found_dict.keys():
        if group_key == "y":
            year = int(found_dict["y"])
            if year <= 68:
                year += 2000
            else:
                year += 1900
        elif group_key == "Y":
            year = int(found_dict["Y"])
        elif group_key == "G":
            iso_year = int(found_dict["G"])
        elif group_key == "m":
            month = int(found_dict["m"])
        elif group_key == "B":
            month = locale_time.f_month.index(found_dict["B"].lower())
        elif group_key == "b":
            month = locale_time.a_month.index(found_dict["b"].lower())
        elif group_key == "d":
            day = int(found_dict["d"])
        elif group_key == "H":
            hour = int(found_dict["H"])
        elif group_key == "I":
            hour = int(found_dict["I"])
            ampm = found_dict.get("p", "").lower()
            if ampm in ("", locale_time.am_pm[0]):
                if hour == 12:
                    hour = 0
            elif ampm == locale_time.am_pm[1]:
                if hour != 12:
                    hour += 12
        elif group_key == "M":
            minute = int(found_dict["M"])
        elif group_key == "S":
            second = int(found_dict["S"])
        elif group_key == "f":
            s = found_dict["f"]
            s += "0" * (6 - len(s))
            fraction = int(s)
        elif group_key == "A":
            weekday = locale_time.f_weekday.index(found_dict["A"].lower())
        elif group_key == "a":
            weekday = locale_time.a_weekday.index(found_dict["a"].lower())
        elif group_key == "w":
            weekday = int(found_dict["w"])
            if weekday == 0:
                weekday = 6
            else:
                weekday -= 1
        elif group_key == "u":
            weekday = int(found_dict["u"])
            weekday -= 1
        elif group_key == "j":
            julian = int(found_dict["j"])
        elif group_key in ("U", "W"):
            week_of_year = int(found_dict[group_key])
            if group_key == "U":
                week_of_year_start = 6
            else:
                week_of_year_start = 0
        elif group_key == "V":
            iso_week = int(found_dict["V"])
        elif group_key == "z":
            z = found_dict["z"]
            if z == "Z":
                gmtoff = 0
            else:
                if z[3] == ":":
                    z = z[:3] + z[4:]
                    if len(z) > 5:
                        if z[5] != ":":
                            msg = f"Inconsistent use of : in {found_dict['z']}"
                            raise ValueError(msg)
                        z = z[:5] + z[6:]
                hours = int(z[1:3])
                minutes = int(z[3:5])
                seconds = int(z[5:7] or 0)
                gmtoff = (hours * 60 * 60) + (minutes * 60) + seconds
                gmtoff_remainder = z[8:]
                gmtoff_remainder_padding = "0" * (6 - len(gmtoff_remainder))
                gmtoff_fraction = int(gmtoff_remainder + gmtoff_remainder_padding)
                if z.startswith("-"):
                    gmtoff = -gmtoff
                    gmtoff_fraction = -gmtoff_fraction
        elif group_key == "Z":
            found_zone = found_dict["Z"].lower()
            for value, tz_values in enumerate(locale_time.timezone):
                if found_zone in tz_values:
                    if (
                        time.tzname[0] == time.tzname[1]
                        and time.daylight
                        and found_zone not in ("utc", "gmt")
                    ):
                        break
                    else:
                        tz = value
                        break

    if iso_year is not None:
        if julian is not None:
            raise ValueError(
                "Day of the year directive '%j' is not "
                "compatible with ISO year directive '%G'. "
                "Use '%Y' instead."
            )
        elif iso_week is None or weekday is None:
            raise ValueError(
                "ISO year directive '%G' must be used with "
                "the ISO week directive '%V' and a weekday "
                "directive ('%A', '%a', '%w', or '%u')."
            )
    elif iso_week is not None:
        if year is None or weekday is None:
            raise ValueError(
                "ISO week directive '%V' must be used with "
                "the ISO year directive '%G' and a weekday "
                "directive ('%A', '%a', '%w', or '%u')."
            )
        else:
            raise ValueError(
                "ISO week directive '%V' is incompatible with "
                "the year directive '%Y'. Use the ISO year '%G' instead."
            )

    leap_year_fix = False
    if year is None:
        if month == 2 and day == 29:
            year = 1904
            leap_year_fix = True
        else:
            year = 1900

    if julian is None and weekday is not None:
        if week_of_year is not None:
            week_starts_Mon = True if week_of_year_start == 0 else False
            julian = _calc_julian_from_U_or_W(
                year, week_of_year, weekday, week_starts_Mon
            )
        elif iso_year is not None and iso_week is not None:
            datetime_result = datetime_date.fromisocalendar(
                iso_year, iso_week, weekday + 1
            )
            year = datetime_result.year
            month = datetime_result.month
            day = datetime_result.day
        if julian is not None and julian <= 0:
            year -= 1
            yday = 366 if calendar.isleap(year) else 365
            julian += yday

    if julian is None:
        julian = (
            datetime_date(year, month, day).toordinal()
            - datetime_date(year, 1, 1).toordinal()
            + 1
        )
    else:
        datetime_result = datetime_date.fromordinal(
            (julian - 1) + datetime_date(year, 1, 1).toordinal()
        )
        year = datetime_result.year
        month = datetime_result.month
        day = datetime_result.day
    if weekday is None:
        weekday = datetime_date(year, month, day).weekday()
    tzname = found_dict.get("Z")

    if leap_year_fix:
        year = 1900

    return (
        (year, month, day, hour, minute, second, weekday, julian, tz, tzname, gmtoff),
        fraction,
        gmtoff_fraction,
    )


def _strptime_time(data_string, format="%a %b %d %H:%M:%S %Y"):
    tt = _strptime(data_string, format)[0]
    return time.struct_time(tt[: time._STRUCT_TM_ITEMS])


def _strptime_datetime(cls, data_string, format="%a %b %d %H:%M:%S %Y"):
    tt, fraction, gmtoff_fraction = _strptime(data_string, format)
    tzname, gmtoff = tt[-2:]
    args = tt[:6] + (fraction,)
    if gmtoff is not None:
        tzdelta = datetime_timedelta(seconds=gmtoff, microseconds=gmtoff_fraction)
        if tzname:
            tz = datetime_timezone(tzdelta, tzname)
        else:
            tz = datetime_timezone(tzdelta)
        args += (tz,)

    return cls(*args)


globals().pop("_require_intrinsic", None)
