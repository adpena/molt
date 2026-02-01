"""Behavior: urllib.parse.urldefrag splits fragment from URL.
Why: URL parsing semantics are core to web stdlib parity.
Pitfalls: Fragment handling must preserve query and base path accurately.
"""

import urllib.parse

print(urllib.parse.urldefrag("http://example.com/a#frag"))
