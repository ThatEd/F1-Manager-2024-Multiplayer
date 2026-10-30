#!/usr/bin/env python3
"""Ask the running game's co-op agent what it serves, and say plainly what a failure means.

Why this exists
---------------
A 404 from the lobby endpoint is ambiguous, and that ambiguity cost a wrong diagnosis. It can mean:

  * the agent is not injected, so nothing is listening (connection refused);
  * something else is listening on the same port;
  * the agent is old and its route table does not have the path;
  * the request path differs from the table's entry (a query string, a trailing slash, a wrong port).

Those are four different fixes. This tool separates them by probing the endpoints the front-end actually
calls, in the order that distinguishes the cases, and printing what each one answered.

Usage
-----
    python tools/agent_routes.py            # probe the live game on this machine
    python tools/agent_routes.py --port 45992
    python tools/agent_routes.py --expect-missing   # self-test: pairs with a planted route removal
"""

import argparse
import json
import socket
import sys

# The literal set of `agent("path", "method")` calls in js/coop/lobby.js and bridge.js. Kept here rather than
# derived so a change to the front-end forces a matching change here - the point is that a human sees both.
FRONT_END_CALLS = [
    ("GET", "/coop/version"),
    ("GET", "/coop/status"),
    ("POST", "/coop/host"),
    ("POST", "/coop/join"),
    ("GET", "/coop/teams"),
    ("POST", "/coop/claim?team=4&role=host"),
    ("GET", "/coop/release?role=host"),
    ("GET", "/coop/relay"),
    ("GET", "/coop/lobby"),
    ("POST", "/coop/lobby/set?state=waiting"),
    ("POST", "/coop/lobby/set?state=idle"),
    ("POST", "/coop/lobby/join"),
    ("POST", "/coop/lobby/invite"),
    ("GET", "/coop/connectlobby"),
    ("GET", "/coop/clock"),
    ("GET", "/coop/inbox"),
]

DEFAULT_PORTS = [45992]


def request(port, method, path, timeout=3.0):
    """One request, returning (status_line, body) or raising with a plain description."""
    req = "%s %s HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n%s\r\n" % (
        method, path,
        "Content-Length: 0\r\n" if method == "POST" else "",
    )
    with socket.create_connection(("127.0.0.1", port), timeout=timeout) as s:
        s.sendall(req.encode())
        chunks = []
        while True:
            b = s.recv(65536)
            if not b:
                break
            chunks.append(b)
    raw = b"".join(chunks).decode("utf-8", "replace")
    head, _, body = raw.partition("\r\n\r\n")
    status = head.split("\r\n")[0] if head else "(no response)"
    return status, body


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, action="append",
                    help="port to probe (repeatable); defaults to 45992")
    ap.add_argument("--expect-missing", action="store_true",
                    help="exit 0 only if at least one endpoint 404s (used to prove this tool notices)")
    args = ap.parse_args()

    ports = args.port or DEFAULT_PORTS
    listening = None
    for port in ports:
        try:
            status, _ = request(port, "GET", "/coop/status", timeout=2.0)
        except (OSError, socket.timeout) as e:
            print("port %d : NOT ANSWERING (%s)" % (port, type(e).__name__))
            continue
        print("port %d : answers, %s" % (port, status))
        listening = port
        break

    if listening is None:
        print()
        print("Nothing is listening on %s." % ", ".join(str(p) for p in ports))
        print("This is NOT a routing problem. In order of likelihood:")
        print("  1. the game is not running")
        print("  2. version.dll is missing next to F1Manager24.exe - it is what loads the agent, and")
        print("     coop_proxy.log beside it records whether it loaded. Reinstalling the mod restores it.")
        print("  3. the agent started but its HTTP thread failed; coop_agent.log should still show")
        print("     'agent loaded into ...' if the DLL was in the process.")
        return 2

    print()
    print("probing the %d endpoints the front-end calls:" % len(FRONT_END_CALLS))
    missing = []
    for method, path in FRONT_END_CALLS:
        try:
            status, body = request(listening, method, path)
        except (OSError, socket.timeout) as e:
            print("  %-6s %-34s ERROR %s" % (method, path, type(e).__name__))
            missing.append((method, path, "error"))
            continue
        code = status.split()[1] if len(status.split()) > 1 else "?"
        if code == "404":
            # The agent echoes what it was asked for, so a 404 body names the exact path it did not match.
            try:
                detail = json.loads(body)
                shown = "%s %s" % (detail.get("method"), detail.get("path"))
            except ValueError:
                shown = body[:120]
            print("  %-6s %-34s 404  (asked for: %s)" % (method, path, shown))
            missing.append((method, path, "404"))
        else:
            print("  %-6s %-34s %s" % (method, path, code))

    print()
    if missing:
        print("%d endpoint(s) did not answer 200:" % len(missing))
        for method, path, why in missing:
            print("   %s %s -> %s" % (method, path, why))
        print()
        print("If the status endpoint answered but these 404, the injected agent predates the route:")
        print("rebuild, redeploy the DLL, and restart the game (a loaded DLL cannot be replaced in-place).")
        if args.expect_missing:
            print("\n--expect-missing: a 404 was required and one was found.")
            return 0
        return 1

    print("all %d endpoints answer 200" % len(FRONT_END_CALLS))
    if args.expect_missing:
        print("\n--expect-missing: FAILED, every endpoint answered")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
