import fromlist_protocol_pkg as package

package.events.append(("body", "broken"))
raise package.failure
