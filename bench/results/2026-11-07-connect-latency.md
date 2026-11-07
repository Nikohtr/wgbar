# Request-to-connect, Connect on Demand (2026-11-07, Nikos-MacBook-Pro, macOS 27)

TCP connect from Python (`socket.create_connection`) to hosts behind `wg-monexo-niko`
(server RTT ~45 ms), with the installed WGBar armed.

| Build | From | Result |
|---|---|---|
| main (armed = no endpoint) | armed | 5111 ms, 5111 ms |
| main | live session | 56–61 ms |
| perf/connect-latency (armed = endpoint, keepalive off) | armed, `bench/connect.sh` ×3 | 58, 58, 58 ms |
| perf/connect-latency | armed, 200 s idle | 60 ms (DNS :53), 58 ms (RDP :3389) |

Full cycle with the installed helper: a connection held 4 s turns the status to `connected`
(keepalive on); 36 s after it closed the status is `armed` again.

A connect that needs a fresh handshake should take ~2 RTT (~100 ms). Not observed here: the
server seems to keep the session alive.
