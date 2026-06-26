"""Purpose: is_private / is_global parity with the full IANA special registry.

Regression for the P1 where ipv4_is_private/is_global covered only a partial set
(10/8, 172.16/12, 192.168/16) and the IPv6 classification used a coarse fc00::/7
check.  CPython 3.12 encodes the full iana-ipv4-special-registry and
iana-ipv6-special-registry, including the 100.64/10 carve-out (both flags False),
the 192.0.0.9/10 exceptions, and IPv4-mapped IPv6 delegation.
"""

import ipaddress

# IPv4 special-purpose registry coverage (one representative per block, plus the
# 192.0.0.9/.10 exceptions and the 100.64/10 "neither" carve-out).
v4_addrs = [
    "0.0.0.0",
    "10.0.0.1",
    "100.64.0.1",  # CGNAT: is_private False AND is_global False
    "100.127.255.255",
    "127.0.0.1",
    "169.254.0.1",
    "172.16.0.1",
    "172.31.255.255",
    "172.32.0.1",  # just outside 172.16/12 -> global
    "192.0.0.1",
    "192.0.0.8",
    "192.0.0.9",  # exception -> global
    "192.0.0.10",  # exception -> global
    "192.0.0.11",
    "192.0.0.170",
    "192.0.0.171",
    "192.0.2.1",
    "192.168.0.1",
    "198.18.0.1",
    "198.19.255.255",
    "198.51.100.1",
    "203.0.113.1",
    "240.0.0.1",
    "255.255.255.255",
    "8.8.8.8",
    "1.1.1.1",
    "9.255.255.255",
    "11.0.0.0",
]
for s in v4_addrs:
    a = ipaddress.IPv4Address(s)
    print(s, a.is_private, a.is_global)

# IPv6 special-purpose registry coverage, including IPv4-mapped delegation and
# the 2001:1::1/2 and 2001:3::/32 exceptions inside 2001::/23.
v6_addrs = [
    "::",
    "::1",
    "::ffff:0:0",
    "::ffff:192.168.1.1",  # mapped -> private (delegates to 192.168.1.1)
    "::ffff:8.8.8.8",  # mapped -> global (delegates to 8.8.8.8)
    "::ffff:100.64.0.1",  # mapped -> neither
    "64:ff9b:1::1",
    "100::1",
    "2001::1",
    "2001:1::1",  # exception -> global
    "2001:1::2",  # exception -> global
    "2001:2::1",
    "2001:3::1",  # exception -> global
    "2001:4:112::1",  # exception -> global
    "2001:20::1",  # exception -> global
    "2001:30::1",  # exception -> global
    "2001:db8::1",
    "2002::1",
    "3fff::1",
    "fc00::1",
    "fd00::1",
    "fe80::1",
    "ff00::1",
    "2620:0:2d0:200::7",  # global unicast
    "8000::",  # upper-half -> global
    "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
]
for s in v6_addrs:
    a = ipaddress.IPv6Address(s)
    print(s, a.is_private, a.is_global)

# ip_address() dispatch + classification end-to-end.
for s in ["192.168.1.1", "8.8.8.8", "2001:db8::1", "2606:4700:4700::1111"]:
    a = ipaddress.ip_address(s)
    print(s, a.version, a.is_private, a.is_global)
