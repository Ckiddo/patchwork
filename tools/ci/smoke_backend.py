"""Exercise the actual local server binary. Uses only temporary test identities.

Run after cargo build -p backend. No production database or remote service is used.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.error
import urllib.request


def stop_gracefully(process):
    if os.name == "nt":
        # Send Ctrl+Break only to the hidden console created for our own child.
        helper = r'''
import ctypes, sys, time
k = ctypes.WinDLL("kernel32", use_last_error=True)
k.FreeConsole()
if not k.AttachConsole(int(sys.argv[1])): sys.exit(2)
handler = ctypes.WINFUNCTYPE(ctypes.c_int, ctypes.c_ulong)(lambda _: 1)
k.SetConsoleCtrlHandler(handler, True)
ok = k.GenerateConsoleCtrlEvent(1, 0)
time.sleep(0.2)
k.FreeConsole()
sys.exit(0 if ok else 3)
'''
        subprocess.run([sys.executable, "-c", helper, str(process.pid)], check=True, timeout=5)
    else:
        process.send_signal(signal.SIGTERM)
    assert process.wait(timeout=15) == 0, "graceful shutdown failed"


def main():
    root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=root / "target/debug" / ("patchwork-server.exe" if os.name == "nt" else "patchwork-server"))
    parser.add_argument("--database-config", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    assert binary.is_file(), "build backend first"
    artifacts = root / "artifacts"
    artifacts.mkdir(exist_ok=True)
    # Temp secrets are deleted on completion, not copied into the evidence report.
    with tempfile.TemporaryDirectory(prefix="backend-smoke-", dir=artifacts) as work:
        work = Path(work)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        key = secrets.token_hex(32)
        (work / "jwt.key").write_text(key, encoding="utf-8")
        config = work / "server.toml"
        config.write_text(f'''listen = "127.0.0.1:{port}"
jwt_secret_file = "jwt.key"
allowed_origins = ["https://ckiddo.github.io"]
workers = 2
shutdown_timeout_secs = 3
log_level = "info"
''', encoding="utf-8")
        credentials = [key]
        if args.database_config:
            source = args.database_config.resolve()
            database = tomllib.loads(source.read_text(encoding="utf-8"))["database"]
            assert database["host"] == "127.0.0.1" and database["name"].startswith("patchwork_test_"), "isolated database required"
            database["password_file"] = (source.parent / database["password_file"]).resolve().as_posix()
            credentials.append(Path(database["password_file"]).read_text(encoding="utf-8").strip())
            with config.open("a", encoding="utf-8") as file:
                file.write("\n[database]\n" + "\n".join(name + " = " + json.dumps(value) for name, value in database.items()) + "\n")
        kwargs = {}
        if os.name == "nt":
            startup = subprocess.STARTUPINFO()
            startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
            startup.wShowWindow = 0
            kwargs = {"startupinfo": startup, "creationflags": subprocess.CREATE_NEW_CONSOLE}
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        base = f"http://127.0.0.1:{port}"

        def request(path, method="GET", body=None, headers=None):
            payload = json.dumps(body).encode() if body is not None else None
            headers = {**(headers or {})}
            if payload is not None:
                headers["Content-Type"] = "application/json"
            req = urllib.request.Request(base + path, data=payload, headers=headers, method=method)
            try:
                response = opener.open(req, timeout=3)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                data = response.read()
                return response.status, response.headers, json.loads(data) if data else None

        with (work / "server.log").open("w", encoding="utf-8") as log:
            process = subprocess.Popen([str(binary), "--config", str(config)], cwd=root, stdout=log, stderr=log, **kwargs)
            try:
                deadline = time.monotonic() + 20
                while True:
                    assert process.poll() is None, "server exited before health probe"
                    try:
                        status, _, _ = request("/healthz")
                        if status == 200:
                            break
                    except OSError:
                        pass
                    assert time.monotonic() < deadline, "health probe timed out"
                    time.sleep(0.1)
                status, _, readiness = request("/readyz")
                assert status == (200 if args.database_config else 503) and readiness["database"] == ("ok" if args.database_config else "not_configured"), "false readiness"
                status, headers, created = request("/api/auth/create", "POST")
                if args.database_config:
                    assert status == 200 and headers["Cache-Control"] == "no-store", "create contract failed"
                    bearer = {"Authorization": "Bearer " + created["jwt"]}
                    status, _, verified = request("/api/auth/verify", "POST", headers=bearer)
                    assert status == 200 and verified["identity"] == created["identity"], "verify contract failed"
                    status, _, updated = request("/api/auth/nickname", "PUT", {"nickname": "测试玩家"}, bearer)
                    assert status == 200 and updated["identity"]["user_id"] == created["identity"]["user_id"], "nickname contract failed"
                    credentials.extend([created["jwt"], updated["jwt"], created["refresh_token"]])
                    from websocket_smoke import check
                    check(base, created["jwt"], request)
                    from websocket_smoke import expiry_check
                    expiry_check(base, key, request, credentials)
                    from friends_smoke import check as check_friends
                    check_friends(base, request, credentials)
                    def restart():
                        nonlocal process
                        process.kill()
                        process.wait(timeout=5)
                        process = subprocess.Popen([str(binary), "--config", str(config)], cwd=root, stdout=log, stderr=log, **kwargs)
                        deadline = time.monotonic() + 20
                        while time.monotonic() < deadline:
                            try:
                                if request("/readyz")[0] == 200:
                                    return
                            except OSError:
                                pass
                            assert process.poll() is None
                            time.sleep(0.1)
                        raise AssertionError("restart did not recover readiness")
                    from recovery_smoke import check as check_recovery, heartbeat_check
                    check_recovery(base, request, credentials, restart)
                    from gameplay_smoke import check as check_gameplay
                    check_gameplay(base, request, credentials, restart)
                    heartbeat_check(base, request, credentials)
                else:
                    assert status == 503 and headers["Cache-Control"] == "no-store", "auth needs database"
                marker = "not-a-real-secret-log-reflection-probe"
                status, _, _ = request("/api/auth/verify?token=" + marker, "POST", headers={"Authorization": "Bearer " + marker})
                assert status == 401, "invalid token was accepted"
                # Different port proves rejection comes from the instance guard.
                other = work / "other.toml"
                other.write_text(config.read_text().replace(f":{port}", ":0"), encoding="utf-8")
                second = subprocess.run([str(binary), "--config", str(other)], cwd=root, capture_output=True, timeout=10, **kwargs)
                assert second.returncode != 0 and b"already running" in second.stderr, "singleton guard failed"
                stop_gracefully(process)
                # Scheduled tasks have no interactive console. Exercise the
                # file signal in a real child, including the relative path.
                content = config.read_text(encoding="utf-8")
                config.write_text('shutdown_signal_file = "backend.stop"\n' + content, encoding="utf-8")
                file_kwargs = dict(kwargs)
                if os.name == "nt":
                    file_kwargs["creationflags"] = subprocess.CREATE_NO_WINDOW
                process = subprocess.Popen([str(binary), "--config", str(config)], cwd=root, stdout=log, stderr=log, **file_kwargs)
                deadline = time.monotonic() + 20
                while True:
                    assert process.poll() is None, "headless server exited early"
                    try:
                        if request("/healthz")[0] == 200:
                            break
                    except OSError:
                        pass
                    assert time.monotonic() < deadline, "headless health probe timed out"
                    time.sleep(0.1)
                (work / "backend.stop").touch()
                assert process.wait(timeout=15) == 0, "file signal did not stop gracefully"
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=5)
        logged = (work / "server.log").read_text(encoding="utf-8")
        for sensitive in credentials + [marker]:
            assert sensitive not in logged, "sensitive data leaked to logs"
        assert '"draining"' in logged and '"stopped"' in logged, "shutdown lifecycle missing"
        with socket.socket() as sock:
            assert sock.connect_ex(("127.0.0.1", port)) != 0, "listener leaked after exit"
        report = {"result": "passed", "checks": ["live persistent auth and WebSocket" if args.database_config else "database-required auth", "honest readiness", "no-store", "singleton", "log redaction", "console and headless file graceful shutdown", "port released"]}
        (artifacts / "backend-smoke.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(report))


if __name__ == "__main__":
    main()
