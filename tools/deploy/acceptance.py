"""T46/T48: two real players, logical restore and PITR on the authorized 5.9 host.

Run Test-Restore.ps1 -Operation Setup first. All writes go to its recorded test DB.
Credentials remain in memory and on the remote host's protected secret files.
"""
import base64
from contextlib import ExitStack
import json
from pathlib import Path
import socket
import subprocess
import sys
import time
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/ci"))
from websockets.sync.client import connect
from friends_smoke import Client, integer, expect_room
from websocket_smoke import field, decode
from gameplay_smoke import check, actor_of, state, synchronized_pair, send_action, committed_pair

SSH = r"D:\download\OpenSSH-Win64\ssh.exe"
PREFIX = "$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue';[Console]::OutputEncoding=[Text.Encoding]::UTF8;\n"


def remote(script, timeout=180):
    encoded = base64.b64encode((PREFIX + script).encode("utf-16le")).decode()
    p = subprocess.run([SSH, "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "word-server-5-9", "powershell.exe", "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded], capture_output=True, timeout=timeout)
    if p.returncode:
        # Scripts suppress private SQL/server values. Keep the detailed diagnostic local.
        (ROOT / "artifacts/t48-last-error.log").write_bytes(p.stdout + p.stderr)
        raise RuntimeError("remote acceptance step failed; inspect artifacts/t48-last-error.log")
    return p.stdout.decode("utf-8-sig").strip()


def operation(name, extra=""):
    result = remote(f"& D:/deploy_patchwork/tools/Test-Restore.ps1 -Operation {name} {extra}")
    return json.loads(result)


def main():
    ctx = operation("Inspect")["Context"]
    assert ctx["Database"].startswith("patchwork_test_deploy_") and ctx["Port"] == 15432
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    tunnel = subprocess.Popen([SSH, "-N", "-T", "-o", "BatchMode=yes", "-o", "ExitOnForwardFailure=yes", "-L", f"127.0.0.1:{port}:127.0.0.1:18121", "word-server-5-9"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    base = f"http://127.0.0.1:{port}"
    url = base.replace("http://", "ws://") + "/api/ws"
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(path, method="GET", body=None, headers=None):
        payload = None if body is None else json.dumps(body).encode()
        with opener.open(urllib.request.Request(base + path, method=method, data=payload, headers=headers or {}), timeout=8) as response:
            return response.status, response.headers, json.load(response)

    def restart():
        operation("Stop")
        operation("Start")

    report = {"Host": "192.168.5.9", "Database": ctx["Database"]}
    success = False
    try:
        deadline = time.monotonic() + 15
        while True:
            try:
                assert request("/readyz")[0] == 200
                break
            except OSError:
                assert time.monotonic() < deadline and tunnel.poll() is None
                time.sleep(.1)
        print("Running full natural game on remote isolated DB...", flush=True)
        check(base, request, [], restart)
        report["CompletedGame"] = operation("Capture")
        assert len(report["CompletedGame"]["results"]) == 1
        print("Restoring logical dump and comparing games, receipts and results...", flush=True)
        report["LogicalRestore"] = operation("Logical")
        print("Creating verified base before the recovery cut point...", flush=True)
        report["Base"] = operation("Base")
        with ExitStack() as stack:
            identities = [request("/api/auth/create", "POST")[2] for _ in range(2)]
            clients = [Client(stack.enter_context(connect(url, origin="https://ckiddo.github.io")), i["jwt"]) for i in identities]
            a, b = clients
            room = expect_room(a.lobby(10, field(1, b"casual") + field(2, b"patchwork_custom_v1")))
            rid = room[1].decode()
            room = expect_room(b.lobby(11, field(1, room[8])))
            room = expect_room(a.lobby(13, integer(1, 1), rid, room[2]))
            room = expect_room(b.lobby(13, integer(1, 1), rid, room[2]))
            game = expect_room(a.lobby(14, room=rid, version=room[2]))[7]
            snapshot = synchronized_pair(clients, game)
            actor = actor_of(state(snapshot))
            version, command = snapshot.get(2, 0), str(uuid.uuid4())
            receipt = send_action(clients[actor], game, version, 10, request_id=command)
            assert 11 in receipt
            snapshot = committed_pair(clients, game, decode(receipt[11])[2])
            saved = state(snapshot)
            expected = operation("Capture")
            target = remote(". D:/deploy_patchwork/tools/Common.ps1;Use-Pg admin;Invoke-Sql \"SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC','YYYY-MM-DD\"\"T\"\"HH24:MI:SS.US\"\"Z\"\"')\"")
            # Ensure the next committed action is strictly beyond the UTC target.
            time.sleep(.2)
            later = send_action(clients[actor_of(saved)], game, snapshot[2], 10)
            assert 11 in later
            committed_pair(clients, game, decode(later[11])[2])
            report["BeforeTarget"] = expected
            report["AfterTarget"] = operation("Capture")
            assert report["AfterTarget"]["receipts"] == expected["receipts"] + 1
            report["TargetTime"] = target
        print("Restoring isolated PGDATA at the UTC cut point...", flush=True)
        report["PITR"] = operation("Recover", "-TargetTime '" + target + "'")
        assert report["PITR"]["State"] == expected, "PITR included/lost an action, event or result"
        with ExitStack() as stack:
            clients = [Client(stack.enter_context(connect(url, origin="https://ckiddo.github.io")), i["jwt"]) for i in identities]
            snapshot = synchronized_pair(clients, game)
            assert state(snapshot) == saved, "original browser identity did not resume recovered game"
            assert send_action(clients[actor], game, version, 10, request_id=command) == receipt
            advanced = send_action(clients[actor_of(saved)], game, snapshot[2], 10)
            assert 11 in advanced, "recovered game did not accept its next real action"
            committed_pair(clients, game, decode(advanced[11])[2])
        report["RecoveredSessionAndNextAction"] = True
        report["Cleanup"] = operation("Cleanup")
        report["Passed"] = True
        success = True
    finally:
        tunnel.terminate()
        tunnel.wait(timeout=10)
        if not success:
            # Leave failed restore evidence; return the normal deployment to service.
            remote("& D:/deploy_patchwork/tools/Test-Restore.ps1 -Operation Stop | Out-Null;Start-ScheduledTask -TaskName PatchworkBackend;. D:/deploy_patchwork/tools/Common.ps1;Wait-Backend")
        (ROOT / "artifacts/t48-acceptance.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"Passed": True, "LogicalSeconds": report["LogicalRestore"]["Seconds"], "PitrSeconds": report["PITR"]["Seconds"], "RecoveredSessionAndNextAction": True}), flush=True)


if __name__ == "__main__":
    main()
