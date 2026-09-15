"""Validate the public API base used by the Pages build and edge probe."""
import argparse
import json
import sys
import urllib.parse


def validate(value: str) -> str:
    """Return a normalized, accepted API base or raise ValueError."""
    if not value or value != value.strip():
        raise ValueError("API base must be a non-empty URL without surrounding whitespace")
    try:
        parsed = urllib.parse.urlsplit(value)
        port = parsed.port
    except ValueError as error:
        raise ValueError("API base has an invalid authority") from error
    if (
        parsed.scheme != "https"
        or not parsed.hostname
        or parsed.path != "/api"
        or parsed.query
        or parsed.fragment
        or parsed.username is not None
        or parsed.password is not None
        or port not in (None, 443)
    ):
        raise ValueError(
            "API base must be HTTPS, contain exactly /api, and have no credentials, query, fragment, or non-443 port"
        )
    # urlsplit preserves a trailing dot in the hostname. It is a valid DNS
    # spelling, but accepting it here would make the configured Origin and
    # Cloudflare hostname differ from the value used by the browser.
    if parsed.hostname.endswith("."):
        raise ValueError("API hostname must not have a trailing dot")
    return value


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("value", nargs="?", default=None)
    args = parser.parse_args()
    value = args.value if args.value is not None else ""
    try:
        accepted = validate(value)
    except ValueError as error:
        print(f"invalid PATCHWORK_API_BASE: {error}", file=sys.stderr)
        return 2
    print(json.dumps({"ApiBase": accepted}, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
