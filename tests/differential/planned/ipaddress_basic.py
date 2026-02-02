"""Purpose: differential coverage for ipaddress basics."""

import ipaddress

v4 = ipaddress.ip_address("192.168.0.1")
v6 = ipaddress.ip_address("2001:db8::1")
net = ipaddress.ip_network("192.168.0.0/24")

print(v4.version, v4.is_private, v4.packed)
print(v6.version, v6.is_private, v6.compressed)
print(net.num_addresses, net.network_address, net.broadcast_address)
