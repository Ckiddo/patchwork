"""Read-only public edge checks. Does not create users, rooms, or change DNS."""
import argparse
import json
from pathlib import Path
import urllib.error
import urllib.parse
import urllib.request

from websockets.sync.client import connect
from websockets.exceptions import InvalidStatus, InvalidStatusCode

from validate_api_base import validate

ORIGIN = "https://ckiddo.github.io"
ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--api-base", required=True)
    args = parser.parse_args()
    try:
        validate(args.api_base)
    except ValueError as error:
        parser.error(str(error))
    parsed = urllib.parse.urlsplit(args.api_base)
    host = "https://" + parsed.netloc
    report = {"ApiBase": args.api_base, "Origin": ORIGIN, "Checks": []}
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(path, origin=ORIGIN, method="GET", extra=None):
        # Cloudflare may challenge the default Python-urllib identity. Use a
        # stable browser-like identity so this read-only probe follows the
        # same edge policy as the Pages client.
        headers = {
            "Origin": origin,
            "User-Agent": (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
                "AppleWebKit/537.36 (KHTML, like Gecko) "
                "Chrome/131.0.0.0 Safari/537.36"
            ),
            **(extra or {}),
        }
        try:
            response = opener.open(urllib.request.Request(host + path, headers=headers, method=method), timeout=20)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            # This probe never sends or records credentials or response bodies.
            return response.status, response.headers

    for _ in range(2):
        status, headers = request("/api/me")
        assert status == 401, "public API did not reach the authentication handler"
        assert "no-store" in headers.get("Cache-Control", "").lower()
        assert headers.get("CF-Cache-Status", "").upper() not in {"HIT", "STALE", "UPDATING", "REVALIDATED"}
        assert headers.get("Access-Control-Allow-Origin") == ORIGIN
        assert headers.get("CF-Ray"), "request did not pass through Cloudflare"
    report["Checks"].append("Cloudflare HTTPS, allowed Origin and repeated uncached authentication response")
    status, headers = request("/api/auth/create", method="OPTIONS", extra={"Access-Control-Request-Method": "POST", "Access-Control-Request-Headers": "authorization,content-type"})
    assert status in (200, 204) and headers.get("Access-Control-Allow-Origin") == ORIGIN
    status, headers = request("/api/me", origin="https://untrusted.example")
    assert headers.get("Access-Control-Allow-Origin") != "https://untrusted.example"
    for path in ("/", "/healthz", "/readyz", "/metrics", "/apix"):
        assert request(path)[0] == 404, "an internal or unmatched path was exposed"
    report["Checks"].append("CORS preflight, untrusted Origin excluded, internal paths return 404")
    ws = "wss://" + parsed.netloc + "/api/ws"
    with connect(ws, origin=ORIGIN, open_timeout=20):
        pass
    for address, origin in ((ws, "https://untrusted.example"), (ws + "?invalid=1", ORIGIN)):
        try:
            with connect(address, origin=origin, open_timeout=20):
                raise AssertionError("WebSocket Origin/query restriction was bypassed")
        except (InvalidStatus, InvalidStatusCode) as error:
            status = getattr(error, "status_code", None) or error.response.status_code
            assert status in (400, 403), "unexpected WebSocket rejection"
    report["Checks"].append("real WSS upgrade; untrusted Origin and query strings rejected")
    report["Passed"] = True
    (ROOT / "artifacts/t50-public-edge.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
