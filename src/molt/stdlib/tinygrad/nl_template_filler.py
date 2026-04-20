"""
Natural Language Template Filler

Parses natural language invoice descriptions and populates template fields.
Handles amounts, dates, client names, line items, currencies, and informal
number expressions.

Examples:
    >>> fill_template_from_nl({}, "Invoice Acme Corp $4,200 for website redesign, due May 15")
    {
        "vendor": "Acme Corp",
        "invoice_number": "auto-generated",
        "items": [{"description": "Website redesign", "qty": 1, "rate": 420000, "amount": 420000}],
        "total": 420000,
        "currency": "USD",
        "due_date": "2026-05-15",
        "payment_terms": "Net 30"
    }
"""

from __future__ import annotations

import re
import uuid
from datetime import date, timedelta
from typing import Any


# ---------------------------------------------------------------------------
# Amount parsing
# ---------------------------------------------------------------------------

_WORD_NUMBERS: dict[str, int] = {
    "zero": 0, "one": 1, "two": 2, "three": 3, "four": 4, "five": 5,
    "six": 6, "seven": 7, "eight": 8, "nine": 9, "ten": 10,
    "eleven": 11, "twelve": 12, "thirteen": 13, "fourteen": 14, "fifteen": 15,
    "sixteen": 16, "seventeen": 17, "eighteen": 18, "nineteen": 19,
    "twenty": 20, "thirty": 30, "forty": 40, "fifty": 50, "sixty": 60,
    "seventy": 70, "eighty": 80, "ninety": 90,
}

_WORD_MULTIPLIERS: dict[str, int] = {
    "hundred": 100,
    "thousand": 1000,
    "grand": 1000,
    "k": 1000,
    "million": 1_000_000,
    "mil": 1_000_000,
    "m": 1_000_000,
    "billion": 1_000_000_000,
    "b": 1_000_000_000,
}

# Currency symbol → ISO code
_CURRENCY_SYMBOLS: dict[str, str] = {
    "$": "USD",
    "€": "EUR",
    "£": "GBP",
    "¥": "JPY",
    "₹": "INR",
    "₩": "KRW",
    "R$": "BRL",
    "A$": "AUD",
    "C$": "CAD",
    "CHF": "CHF",
}

# Pattern: optional currency symbol, digits with optional commas/decimals, optional suffix
_AMOUNT_PATTERN = re.compile(
    r"(?P<symbol>[€£¥₹₩]|\$|R\$|A\$|C\$|CHF\s?)?"
    r"(?P<number>[\d,]+(?:\.\d{1,2})?)"
    r"(?P<suffix>[kKmMbB])?"
)

# Pattern for word-based amounts like "five grand", "twenty thousand"
_WORD_AMOUNT_PATTERN = re.compile(
    r"\b(?P<words>(?:" + "|".join(_WORD_NUMBERS.keys()) + r")(?:\s+(?:" +
    "|".join(_WORD_NUMBERS.keys()) + r"))*)"
    r"(?:\s+(?P<multiplier>" + "|".join(
        k for k in _WORD_MULTIPLIERS.keys() if len(k) > 1
    ) + r"))?\b",
    re.IGNORECASE,
)


def parse_amount_cents(text: str) -> tuple[int, str] | None:
    """
    Parse an amount from text, returning (cents, currency_code) or None.

    Handles:
        "$5K" → (500000, "USD")
        "$4,200" → (420000, "USD")
        "$4,200.50" → (420050, "USD")
        "€1000" → (100000, "EUR")
        "five grand" → (500000, "USD")  (defaults to USD for word amounts)
    """
    # Try symbolic amounts first
    m = _AMOUNT_PATTERN.search(text)
    if m and m.group("number"):
        symbol = (m.group("symbol") or "").strip()
        currency = _CURRENCY_SYMBOLS.get(symbol, "USD")
        raw = m.group("number").replace(",", "")
        value = float(raw)
        suffix = (m.group("suffix") or "").lower()
        if suffix in _WORD_MULTIPLIERS:
            value *= _WORD_MULTIPLIERS[suffix]
        cents = round(value * 100)
        return (cents, currency)

    # Try word-based amounts
    m = _WORD_AMOUNT_PATTERN.search(text)
    if m:
        words = m.group("words").lower().split()
        value = 0
        current = 0
        for w in words:
            if w in _WORD_NUMBERS:
                current += _WORD_NUMBERS[w]
        value = current
        multiplier_word = (m.group("multiplier") or "").lower()
        if multiplier_word in _WORD_MULTIPLIERS:
            value *= _WORD_MULTIPLIERS[multiplier_word]
        if value > 0:
            return (value * 100, "USD")

    return None


def parse_all_amounts(text: str) -> list[tuple[int, str, int, int]]:
    """
    Find all amounts in text. Returns list of (cents, currency, start, end).
    """
    results = []
    for m in _AMOUNT_PATTERN.finditer(text):
        if not m.group("number"):
            continue
        symbol = (m.group("symbol") or "").strip()
        currency = _CURRENCY_SYMBOLS.get(symbol, "USD")
        raw = m.group("number").replace(",", "")
        value = float(raw)
        suffix = (m.group("suffix") or "").lower()
        if suffix in _WORD_MULTIPLIERS:
            value *= _WORD_MULTIPLIERS[suffix]
        cents = round(value * 100)
        results.append((cents, currency, m.start(), m.end()))
    return results


# ---------------------------------------------------------------------------
# Date parsing
# ---------------------------------------------------------------------------

_MONTHS: dict[str, int] = {
    "january": 1, "jan": 1, "february": 2, "feb": 2, "march": 3, "mar": 3,
    "april": 4, "apr": 4, "may": 5, "june": 6, "jun": 6, "july": 7, "jul": 7,
    "august": 8, "aug": 8, "september": 9, "sep": 9, "sept": 9,
    "october": 10, "oct": 10, "november": 11, "nov": 11, "december": 12, "dec": 12,
}

# "May 15", "May 15, 2026", "May 15th", "15 May 2026"
_DATE_PATTERN = re.compile(
    r"\b(?:(?P<month_name>" + "|".join(_MONTHS.keys()) + r")\s+(?P<day>\d{1,2})(?:st|nd|rd|th)?"
    r"(?:[,\s]+(?P<year>\d{4}))?|"
    r"(?P<day2>\d{1,2})(?:st|nd|rd|th)?\s+(?P<month_name2>" + "|".join(_MONTHS.keys()) + r")"
    r"(?:[,\s]+(?P<year2>\d{4}))?)\b",
    re.IGNORECASE,
)

# "Net 30", "Net 60", "Net 15"
_NET_TERMS_PATTERN = re.compile(r"\bNet\s+(\d+)\b", re.IGNORECASE)

# ISO dates: 2026-05-15
_ISO_DATE_PATTERN = re.compile(r"\b(\d{4})-(\d{2})-(\d{2})\b")

# Relative: "in 30 days", "within 14 days"
_RELATIVE_DATE_PATTERN = re.compile(r"\b(?:in|within)\s+(\d+)\s+days?\b", re.IGNORECASE)


def parse_date(text: str, reference_date: date | None = None) -> date | None:
    """Parse an explicit date from text."""
    ref = reference_date or date.today()

    # ISO date
    m = _ISO_DATE_PATTERN.search(text)
    if m:
        return date(int(m.group(1)), int(m.group(2)), int(m.group(3)))

    # Named month date
    m = _DATE_PATTERN.search(text)
    if m:
        if m.group("month_name"):
            month = _MONTHS[m.group("month_name").lower()]
            day = int(m.group("day"))
            year = int(m.group("year")) if m.group("year") else ref.year
        else:
            month = _MONTHS[m.group("month_name2").lower()]
            day = int(m.group("day2"))
            year = int(m.group("year2")) if m.group("year2") else ref.year
        result = date(year, month, day)
        # If the date is in the past this year, assume next year
        if result < ref and not (m.group("year") or m.group("year2")):
            result = date(year + 1, month, day)
        return result

    return None


def parse_payment_terms(text: str) -> tuple[str | None, int | None]:
    """
    Parse payment terms. Returns (terms_string, net_days) or (None, None).
    """
    m = _NET_TERMS_PATTERN.search(text)
    if m:
        days = int(m.group(1))
        return (f"Net {days}", days)

    m = _RELATIVE_DATE_PATTERN.search(text)
    if m:
        days = int(m.group(1))
        return (f"Net {days}", days)

    # Common terms
    for term, days in [("due on receipt", 0), ("cod", 0), ("prepaid", 0)]:
        if term in text.lower():
            return (term.title(), days)

    return (None, None)


# ---------------------------------------------------------------------------
# Client / vendor parsing
# ---------------------------------------------------------------------------

# Trigger words that precede the client name
_CLIENT_TRIGGERS = re.compile(
    r"\b(?:invoice|bill|for|to|from|client|vendor|company|billed?\s+to)\s+",
    re.IGNORECASE,
)

# Words that are NOT part of a client name (stop words)
_STOP_WORDS = {
    "for", "due", "net", "total", "amount", "at", "on", "the", "a", "an",
    "in", "by", "with", "and", "or", "is", "was", "of",
}


def parse_client_name(text: str) -> str | None:
    """
    Extract client/vendor name from natural language.

    Patterns:
        "Bill Acme" → "Acme"
        "Invoice Widget Corp" → "Widget Corp"
        "Invoice Acme Corp $4,200 for ..." → "Acme Corp"
    """
    for m in _CLIENT_TRIGGERS.finditer(text):
        rest = text[m.end():].strip()
        # Collect capitalized words (the client name)
        words = []
        for token in rest.split():
            # Stop at amounts, dates, or lowercase stop words
            clean = token.rstrip(".,;:")
            if clean.lower() in _STOP_WORDS and words:
                break
            if clean.startswith("$") or clean.startswith("€") or clean.startswith("£"):
                break
            if re.match(r"^\d", clean) and words:
                break
            if clean[0:1].isupper() or clean.lower() in ("inc", "llc", "ltd", "corp", "co", "inc.", "llc.", "ltd.", "corp."):
                words.append(clean)
            elif not words:
                # First word after trigger might be lowercase for short names
                words.append(clean.title())
            else:
                break
        if words:
            return " ".join(words)

    return None


# ---------------------------------------------------------------------------
# Line item parsing
# ---------------------------------------------------------------------------

_ITEM_PATTERN = re.compile(
    r"\bfor\s+(.+?)(?:\s*,|\s+due\b|\s+net\b|\s+at\b|\s+on\b|$)",
    re.IGNORECASE,
)


def parse_line_items(text: str, total_cents: int | None) -> list[dict[str, Any]]:
    """
    Extract line items from the utterance.

    Returns a list of dicts with keys: description, qty, rate, amount (all amounts in cents).
    """
    items = []
    m = _ITEM_PATTERN.search(text)
    if m:
        desc_raw = m.group(1).strip().rstrip(".,;:")
        # Clean up: remove amount references already captured
        desc_clean = _AMOUNT_PATTERN.sub("", desc_raw).strip()
        if not desc_clean:
            desc_clean = desc_raw
        # Capitalize first letter
        desc_clean = desc_clean[0].upper() + desc_clean[1:] if desc_clean else desc_clean
        amount = total_cents or 0
        items.append({
            "description": desc_clean,
            "qty": 1,
            "rate": amount,
            "amount": amount,
        })

    if not items and total_cents:
        items.append({
            "description": "Services",
            "qty": 1,
            "rate": total_cents,
            "amount": total_cents,
        })

    return items


# ---------------------------------------------------------------------------
# Invoice number generation
# ---------------------------------------------------------------------------

def generate_invoice_number() -> str:
    """Generate a short unique invoice number."""
    return f"INV-{uuid.uuid4().hex[:8].upper()}"


# ---------------------------------------------------------------------------
# Main entry point
# ---------------------------------------------------------------------------

def fill_template_from_nl(
    template: dict[str, Any],
    utterance: str,
    context: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """
    Fill a template's fields from a natural language description.

    Args:
        template: Template definition (sections, fields, styles). Currently used
            for pass-through of non-fillable fields.
        utterance: Natural language like "Bill Acme $5K for redesign, Net 30"
        context: Optional user context with keys like 'currency', 'locale',
            'issue_date', 'recent_clients'.

    Returns:
        Filled template with populated fields. All monetary amounts are in cents.
    """
    ctx = context or {}
    ref_date = ctx.get("issue_date", date.today())
    if isinstance(ref_date, str):
        ref_date = date.fromisoformat(ref_date)

    result: dict[str, Any] = {}

    # 1. Parse amount and currency
    amount_result = parse_amount_cents(utterance)
    if amount_result:
        total_cents, currency = amount_result
    else:
        total_cents = 0
        currency = ctx.get("currency", "USD")

    # Override currency from context if provided and no explicit symbol
    if "currency" in ctx and not any(sym in utterance for sym in _CURRENCY_SYMBOLS):
        currency = ctx["currency"]

    result["currency"] = currency
    result["total"] = total_cents

    # 2. Parse client name
    vendor = parse_client_name(utterance)
    # Fall back to recent clients from context
    if not vendor and "recent_clients" in ctx:
        clients = ctx["recent_clients"]
        for client in clients:
            if client.lower() in utterance.lower():
                vendor = client
                break
    result["vendor"] = vendor or ""

    # 3. Parse dates and payment terms
    terms_str, net_days = parse_payment_terms(utterance)
    explicit_date = parse_date(utterance, reference_date=ref_date)

    if explicit_date:
        result["due_date"] = explicit_date.isoformat()
    elif net_days is not None:
        result["due_date"] = (ref_date + timedelta(days=net_days)).isoformat()
    else:
        # Default: Net 30
        result["due_date"] = (ref_date + timedelta(days=30)).isoformat()

    result["issue_date"] = ref_date.isoformat()
    result["payment_terms"] = terms_str or "Net 30"

    # 4. Parse line items
    result["items"] = parse_line_items(utterance, total_cents)

    # 5. Invoice number
    result["invoice_number"] = template.get("invoice_number") or generate_invoice_number()

    # 6. Pass through any template fields we didn't fill
    for key, value in template.items():
        if key not in result:
            result[key] = value

    return result
