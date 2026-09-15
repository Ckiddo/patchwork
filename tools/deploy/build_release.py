"""Package an already-built Windows release with binary and source fingerprints."""
import datetime as dt
import hashlib
import json
from pathlib import Path
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[2]


def main():
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT))
    candidates = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=ROOT).decode().split("\0")
    sources = []
    for name in sorted(set(candidates)):
        path = ROOT / name
        if name and path.is_file() and (name in ("Cargo.toml", "Cargo.lock") or name.startswith(("backend/", "game_core/", "util_lib/", "tools/deploy/"))):
            sources.append({"path": name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
    source_bytes = (json.dumps(sources, indent=2) + "\n").encode()
    fingerprint = hashlib.sha256(source_bytes).hexdigest()
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    release = f"{stamp}-{commit[:8]}-{'dirty-' if dirty else ''}{fingerprint[:8]}"
    payload = {name: (ROOT / "target/release" / name).read_bytes() for name in ("patchwork-server.exe", "patchwork-migrate.exe")}
    payload["source-manifest.json"] = source_bytes
    schema = []
    for sql in sorted((ROOT / "backend/migrations").glob("*.sql")):
        data = sql.read_bytes()
        payload["migrations/" + sql.name] = data
        schema.append({"version": int(sql.name.split("_")[0]), "checksum": hashlib.sha384(data).hexdigest()})
    manifest = {"Project": "patchwork", "Release": release, "BuiltUtc": stamp, "BaseCommit": commit, "Dirty": dirty,
                "SourceSha256": fingerprint, "PostgresMajor": 18, "Schema": schema,
                "Files": [{"Path": name, "Bytes": len(data), "Sha256": hashlib.sha256(data).hexdigest()} for name, data in payload.items()]}
    payload["manifest.json"] = (json.dumps(manifest, indent=2) + "\n").encode()
    output = ROOT / "artifacts" / f"{release}.zip"
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, data in payload.items():
            archive.writestr(name, data)
    report = {"Release": release, "Zip": str(output), "Sha256": hashlib.sha256(output.read_bytes()).hexdigest(), "Bytes": output.stat().st_size}
    (ROOT / "artifacts/release-package.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
