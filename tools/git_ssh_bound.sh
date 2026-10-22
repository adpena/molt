#!/bin/sh
# Bound one ssh connection of a Git transfer by its progress (tools/git_transfer.py).
#
# Usage: sh git_ssh_bound.sh STALL_SECONDS UNATTENDED SSH_PROGRAM [ARG...]
# The arguments after SSH_PROGRAM are the user's own ssh options, then Git's.
#
# OpenSSH keeps the first value it obtains for an option: the command line,
# then ssh_config. `ssh -G` prints the value this connection would use. Each
# bound is added in front of the user's arguments only where that value is
# unset or looser than STALL_SECONDS, so a tighter user value always stays.
# ConnectTimeout covers the TCP connect and the banner exchange; server-alive
# probes cover the session after it. BatchMode is added only for an
# unattended transfer, where no person can answer a prompt.
stall=$1
unattended=$2
program=$3
shift 3

effective=$("$program" -G "$@" 2>/dev/null) || exec "$program" "$@"

value() {
    printf '%s\n' "$effective" | awk -v key="$1" '$1 == key { print $2; exit }'
}

connect=$(value connecttimeout)
case $connect in
'' | none) set -- -o "ConnectTimeout=$stall" "$@" ;;
*) if [ "$connect" -gt "$stall" ]; then set -- -o "ConnectTimeout=$stall" "$@"; fi ;;
esac

interval=$(value serveraliveinterval)
count=$(value serveralivecountmax)
if [ "${interval:-0}" -eq 0 ] || [ $((interval * ${count:-3})) -gt "$stall" ]; then
    set -- -o "ServerAliveInterval=$((stall / 4))" -o ServerAliveCountMax=4 "$@"
fi

if [ "$unattended" = 1 ] && [ "$(value batchmode)" != yes ]; then
    set -- -o BatchMode=yes "$@"
fi

exec "$program" "$@"
