"""Read deployment metadata with the existing Git credential helper, without logging credentials."""
import json
import os
from pathlib import Path
import subprocess
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[2]


def main():
    env = {**os.environ, "GIT_TERMINAL_PROMPT": "0", "GCM_INTERACTIVE": "Never"}
    result = subprocess.run(["git", "credential", "fill"], input="protocol=https\nhost=github.com\n\n", capture_output=True, text=True, env=env, timeout=25)
    values = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line) if result.returncode == 0 else {}
    token = values.get("password")
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "patchwork-deployment"}
    if token:
        headers["Authorization"] = "Bearer " + token

    def get(path):
        for proxy in ({}, {"https": "http://127.0.0.1:7897"}):
            try:
                opener = urllib.request.build_opener(urllib.request.ProxyHandler(proxy))
                with opener.open(urllib.request.Request("https://api.github.com/repos/Ckiddo/patchwork" + path, headers=headers), timeout=20) as response:
                    return response.status, json.load(response)
            except urllib.error.HTTPError as error:
                return error.code, {}
            except OSError:
                if proxy:
                    return "network_error", {}

    code, repo = get("")
    pc, pages = get("/pages")
    vc, variable = get("/actions/variables/PATCHWORK_API_BASE")
    bc, branch = get("/branches/main")
    report = {"CredentialAvailable": bool(token), "RepositoryStatus": code, "DefaultBranch": repo.get("default_branch"), "Permissions": repo.get("permissions"), "PagesStatus": pc, "Pages": {k: pages.get(k) for k in ("html_url", "status", "build_type", "source")}, "ApiVariableStatus": vc, "ApiBase": variable.get("value"), "MainStatus": bc, "MainSha": branch.get("commit", {}).get("sha")}
    (ROOT / "artifacts/github-preflight.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
