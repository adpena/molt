"""Purpose: differential coverage for html escape/unescape basic."""

import html

# Basic escape
print(html.escape("<p>hello</p>"))
print(html.escape('<p>"hello"</p>'))
print(html.escape("a & b"))
print(html.escape("no special chars"))

# Escape with quote=False
print(html.escape('<p>"hello"</p>', quote=False))
print(html.escape("it's a test", quote=False))

# Escape with quote=True (default)
print(html.escape("it's a test"))
print(html.escape('"double" & \'single\''))

# Unescape named references
print(html.unescape("&amp;"))
print(html.unescape("&lt;p&gt;"))
print(html.unescape("&quot;hello&quot;"))

# Unescape numeric references
print(html.unescape("&#62;"))
print(html.unescape("&#x3e;"))
print(html.unescape("&#38;"))

# Unescape mixed
print(html.unescape("&lt;p&gt;hello &amp; &#62; world&lt;/p&gt;"))

# Empty/no-op
print(html.escape(""))
print(html.unescape(""))
print(html.unescape("no entities here"))

# Round-trip
original = '<script>alert("xss")</script>'
escaped = html.escape(original)
print(escaped)
print(html.unescape(escaped))
