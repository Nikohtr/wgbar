#!/bin/bash
# Request-to-connect time through Connect on Demand: waits until the installed WGBar has the
# tunnel armed (not connected), then times one TCP connect into it, RUNS times.
# Usage: bench/connect.sh <tunnel> <host> <port> [runs=3]
#   e.g. bench/connect.sh wg-office 10.42.1.63 3389
# Needs the helper (./sudoers.sh) and WGBar running with Connect on Demand on. Each run waits
# for WGBar's idle-out (30 s by default) after the previous one.
set -euo pipefail
TUNNEL=$1 HOST=$2 PORT=$3 RUNS=${4:-3}
status() { sudo -n /usr/local/libexec/wgbar-helper status "$TUNNEL"; }
for ((i = 1; i <= RUNS; i++)); do
  for ((w = 0; ; w++)); do
    s=$(status)
    [[ $s != connected ]] && break
    ((w < 300)) || { echo "still connected after 300 s; is something using the tunnel?" >&2; exit 1; }
    sleep 1
  done
  # Apple's python3: a TCP connect with a monotonic clock around it.
  ms=$(/usr/bin/python3 -I -c '
import socket, sys, time
t = time.monotonic()
socket.create_connection((sys.argv[1], int(sys.argv[2])), timeout=30).close()
print(round((time.monotonic() - t) * 1000))' "$HOST" "$PORT")
  echo "run $i: ${ms} ms (from $s)"
done
