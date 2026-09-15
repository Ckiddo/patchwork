"""Download the official Windows connector, checking the release asset digest."""
import hashlib
import json
from pathlib import Path
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "artifacts/cloudflared"


def fetch(url):
    for proxy in ({}, {"https": "http://127.0.0.1:7897"}):
        try:
            opener = urllib.request.build_opener(urllib.request.ProxyHandler(proxy))
            with opener.open(urllib.request.Request(url, headers={"User-Agent": "patchwork-deployment"}), timeout=45) as response:
                began, chunks, size = time.monotonic(), [], 0
                while chunk := response.read(256 * 1024):
                    chunks.append(chunk)
                    size += len(chunk)
                    if size % (4 * 1024 * 1024) == 0:
                        print(f"Downloaded {size // (1024 * 1024)} MiB ({'proxy' if proxy else 'direct'})", flush=True)
                    if time.monotonic() - began > 90:
                        raise TimeoutError("transfer deadline")
                return b"".join(chunks)
        except OSError:
            if proxy:
                raise RuntimeError("Official release download failed directly and through the local proxy") from None


def main():
    release = json.loads(fetch("https://api.github.com/repos/cloudflare/cloudflared/releases/latest"))
    assert not release["draft"] and not release["prerelease"]
    asset = next(a for a in release["assets"] if a["name"] == "cloudflared-windows-amd64.exe")
    expected = asset.get("digest", "")
    assert expected.startswith("sha256:"), "Official asset digest is required"
    binary = fetch(asset["browser_download_url"])
    digest = hashlib.sha256(binary).hexdigest()
    assert "sha256:" + digest == expected and len(binary) == asset["size"]
    DEST.mkdir(parents=True, exist_ok=True)
    (DEST / "cloudflared.exe").write_bytes(binary)
    manifest = {"Project": "cloudflared", "Version": release["tag_name"], "Sha256": digest, "Bytes": len(binary), "Source": asset["browser_download_url"]}
    (DEST / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(manifest))


if __name__ == "__main__":
    main()
