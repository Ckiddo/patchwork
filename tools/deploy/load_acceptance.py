"""T53: 100 live WebSockets, 50 games and one real process crash on isolated 5.9 DB.

Run Test-Restore.ps1 Setup first. A barrier proves all 100 authenticated sockets
are simultaneously open before actions begin. No production rows are written.
"""
import base64
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack
import datetime as dt
import json
import math
from pathlib import Path
import socket
import subprocess
import threading
import time
import urllib.request

from acceptance import ROOT, SSH, PREFIX, operation, remote
from friends_smoke import Client, integer, expect_room
from gameplay_smoke import actor_of, state, synchronized_pair, send_action, committed_pair
from websocket_smoke import field, decode
from websockets.sync.client import connect


def percentile(values, p):
    return sorted(values)[max(0, math.ceil(len(values) * p) - 1)] if values else None


def main():
    ctx = operation("Inspect")["Context"]
    assert ctx["Database"].startswith("patchwork_test_deploy_") and ctx["Port"] == 15432
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    tunnel = subprocess.Popen([SSH, "-N", "-T", "-o", "BatchMode=yes", "-o", "ExitOnForwardFailure=yes", "-L", f"127.0.0.1:{port}:127.0.0.1:18121", "word-server-5-9"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    base = f"http://127.0.0.1:{port}"
    url = f"ws://127.0.0.1:{port}/api/ws"
    barrier = threading.Barrier(51, timeout=75)
    report = {"Host": "192.168.5.9", "Database": ctx["Database"], "Transport": "SSH loopback forwarding; not Cloudflare", "TargetConnections": 100, "TargetRooms": 50, "StartedUtc": dt.datetime.now(dt.timezone.utc).isoformat(), "ActionPauseSeconds": .15, "Failures": []}
    monitor = None

    def save_report():
        (ROOT / "artifacts/t53-load.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    def request(path, method="GET"):
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open(urllib.request.Request(base + path, method=method), timeout=15) as response:
            return json.load(response)

    def run_room(index):
        delays, recovered = [], None
        stage = "authentication"
        try:
            with ExitStack() as stack:
                identities = [request("/api/auth/create", "POST") for _ in range(2)]

                def connections():
                    return [Client(stack.enter_context(connect(url, origin="https://ckiddo.github.io", open_timeout=15)), identity["jwt"]) for identity in identities]

                clients = connections()
                barrier.wait()  # Finish the connection burst before creating rooms.
                stage = "room_create"
                a, b = clients
                room = expect_room(a.lobby(10, field(1, b"casual") + field(2, b"patchwork_custom_v1")))
                rid = room[1].decode()
                stage = "room_join"
                room = expect_room(b.lobby(11, field(1, room[8])))
                # Initial presence reconciliation may bump a newly joined room's
                # version. Wait one tick and explicitly read the current version.
                time.sleep(1.2)
                stage = "room_ready"
                room = expect_room(a.lobby(16, room=rid))
                room = expect_room(a.lobby(13, integer(1, 1), rid, room[2]))
                room = expect_room(b.lobby(13, integer(1, 1), rid, room[2]))
                stage = "room_start"
                game = expect_room(a.lobby(14, room=rid, version=room[2]))[7]
                stage = "initial_sync"
                snapshot = synchronized_pair(clients, game)
                barrier.wait()  # 50 initialized games, all 100 sockets remain open.
                stage = "actions_before_crash"

                def action(current):
                    value = state(current)
                    actor = actor_of(value)
                    tag, body = 10, b""
                    if value["action"]["pending_specials"]:
                        cells = value["players"][actor]["board"]["cells"]
                        x, y = next((x, y) for y in range(9) for x in range(9) if cells[y][x] is None)
                        tag, body = 12, integer(1, x << 1) + integer(2, y << 1)
                    started = time.perf_counter()
                    receipt = send_action(clients[actor], game, current.get(2, 0), tag, body)
                    assert 11 in receipt, f"action error code {decode(receipt.get(10, b'')).get(1)}"
                    updated = committed_pair(clients, game, decode(receipt[11]).get(2, 0))
                    delays.append((time.perf_counter() - started) * 1000)
                    time.sleep(.15)
                    return updated

                for _ in range(20):
                    snapshot = action(snapshot)
                saved = state(snapshot)
                barrier.wait()  # All rooms have durable actions before the crash.
                barrier.wait()  # Main thread kills and restarts the actual process.
                began = time.perf_counter()
                stage = "recovery"
                clients = connections()
                snapshot = synchronized_pair(clients, game)
                assert state(snapshot) == saved, "crash recovery changed committed game state"
                recovered = (time.perf_counter() - began) * 1000
                barrier.wait()  # Re-establish all 100 sockets before finishing games.
                stage = "actions_after_crash"
                for _ in range(95):
                    if state(snapshot)["result"] is not None:
                        break
                    snapshot = action(snapshot)
                final = state(snapshot)
                assert final["lifecycle"] == "finished" and [p["time_position"] for p in final["players"]] == [53, 53]
                return {"Room": index, "Actions": len(delays), "ActionMilliseconds": delays, "RecoveryMilliseconds": recovered, "Finished": True}
        except Exception as error:
            if not isinstance(error, threading.BrokenBarrierError):
                report["Failures"].append({"Room": index, "Stage": stage, "Type": type(error).__name__, "Assertion": str(error) if isinstance(error, AssertionError) else None})
            barrier.abort()
            raise

    try:
        deadline = time.monotonic() + 15
        while True:
            try:
                assert request("/readyz")["database"] == "ok"
                break
            except OSError:
                assert time.monotonic() < deadline and tunnel.poll() is None
                time.sleep(.1)
        encoded = base64.b64encode((PREFIX + "& D:/deploy_patchwork/tools/Measure-Load.ps1 -Seconds 180").encode("utf-16le")).decode()
        monitor = subprocess.Popen([SSH, "-T", "-o", "BatchMode=yes", "word-server-5-9", "powershell.exe", "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        print("Opening 100 authenticated connections and 50 independent games...", flush=True)
        with ThreadPoolExecutor(max_workers=50) as pool:
            futures = [pool.submit(run_room, i) for i in range(50)]
            try:
                barrier.wait()
                report["SimultaneousAuthenticatedConnections"] = 100
                print("All 100 authenticated sockets are connected; initializing 50 rooms...", flush=True)
                barrier.wait()
                print("100 connections / 50 running games established; sending concurrent committed actions...", flush=True)
                barrier.wait()
                started = time.perf_counter()
                print("Crashing the isolated backend after 1,000 durable actions...", flush=True)
                crash = json.loads(remote("""
$p=@(Get-NetTCPConnection -State Listen -LocalPort 18121).OwningProcess | Select-Object -Unique
if(@($p).Count -ne 1){throw 'Unexpected acceptance listener'}
$process=Get-Process -Id $p
if($process.Path -notlike 'D:\\deploy_patchwork\\releases\\*\\patchwork-server.exe'){throw 'Unexpected process path'}
Stop-Process -Id $p -Force
& D:/deploy_patchwork/tools/Test-Restore.ps1 -Operation Stop | Out-Null
& D:/deploy_patchwork/tools/Test-Restore.ps1 -Operation Start | Out-Null
$next=@(Get-NetTCPConnection -State Listen -LocalPort 18121).OwningProcess | Select-Object -Unique
@{OldPid=$p;NewPid=$next}|ConvertTo-Json
"""))
                report["Crash"] = {**crash, "ReadySeconds": time.perf_counter() - started}
                barrier.wait()
                barrier.wait()
                print("All 50 games recovered unchanged; finishing games with 100 simultaneous connections...", flush=True)
                results = [f.result() for f in futures]
            except Exception:
                barrier.abort()
                # Include only stable exception types, never protocol payloads or tokens.
                save_report()
                raise
        delays = [d for room in results for d in room["ActionMilliseconds"]]
        report.update({"FinishedGames": len(results), "CommittedActions": len(delays), "ActionMs": {"p50": percentile(delays, .50), "p95": percentile(delays, .95), "p99": percentile(delays, .99), "max": max(delays)}, "RecoveryMs": {"p95": percentile([r["RecoveryMilliseconds"] for r in results], .95), "max": max(r["RecoveryMilliseconds"] for r in results)}, "Errors": 0, "Passed": True})
    finally:
        save_report()
        if monitor:
            remote("$c=Get-Content D:/deploy_patchwork/control/acceptance-context.json -Raw|ConvertFrom-Json;. D:/deploy_patchwork/tools/Common.ps1;Write-Utf8 ((Assert-ProjectPath $c.Directory)+'\\load-monitor.stop') 'stop'")
            output, errors = monitor.communicate(timeout=20)
            samples = [json.loads(line) for line in output.decode("utf-8-sig").splitlines() if line.strip()]
            (ROOT / "artifacts/t53-samples.json").write_text(json.dumps(samples, indent=2) + "\n", encoding="utf-8")
            report["Samples"] = len(samples)
            if samples:
                cpu = []
                previous = {}
                memory = []
                for sample in samples:
                    at = dt.datetime.fromisoformat(sample["At"].replace("Z", "+00:00")).timestamp()
                    for process in sample["Backend"]:
                        memory.append(process["WorkingSetBytes"])
                        prior = previous.get(process["Pid"])
                        if prior and at > prior[0]:
                            cpu.append(100 * (process["CpuSeconds"] - prior[1]) / (at - prior[0]) / sample["LogicalProcessors"])
                        previous[process["Pid"]] = (at, process["CpuSeconds"])
                report["Resources"] = {"BackendCpuPercentOfHostMax": max(cpu, default=0), "BackendCpuPercentOfHostP95": percentile(cpu, .95), "WorkingSetMiBMax": max(memory, default=0) / 1024**2, "DatabaseConnectionsMax": max(s["Database"]["connections"] for s in samples), "LockWaiterSamples": sum(s["Database"]["lock_waiters"] > 0 for s in samples), "LwLockWaiterSamples": sum(s["Database"]["lwlock_waiters"] > 0 for s in samples), "DeadlocksDelta": samples[-1]["Database"]["deadlocks"] - samples[0]["Database"]["deadlocks"]}
            report["MonitorExitCode"] = monitor.returncode
            if errors or monitor.returncode:
                (ROOT / "artifacts/t53-monitor-error.log").write_bytes(errors)
                report["Passed"] = False
        tunnel.terminate()
        tunnel.wait(timeout=10)
        report["StateBeforeCleanup"] = operation("Capture")
        save_report()
        try:
            stop_started = time.perf_counter()
            operation("Stop")
            report["ShutdownSeconds"] = time.perf_counter() - stop_started
            report["PoolAcquireMs"] = json.loads(remote(r"""
$ctx=Get-Content D:/deploy_patchwork/control/acceptance-context.json -Raw|ConvertFrom-Json
$start='acceptance-'+[DateTime]::ParseExact($ctx.Id,'yyyyMMddHHmmss',[Globalization.CultureInfo]::InvariantCulture).ToString('yyyyMMddTHHmmss')
$logs=@(Get-ChildItem D:/deploy_patchwork/logs/backend -Filter 'acceptance-*.out.log'|Where-Object {$_.Name -ge $start})
$summaries=@(foreach($log in ($logs|Sort-Object Name)){
    foreach($line in (Select-String -LiteralPath $log.FullName -SimpleMatch '"event":"pool_acquire_summary"')){
        ($line.Line|ConvertFrom-Json).fields
    }
})
if(!$summaries.Count){throw 'Pool acquisition telemetry was not captured'}
$last=$summaries[-1]
if($last.discarded -ne 0){throw 'Pool timing buffer overflowed'}
@{Samples=$last.samples;P95=$last.p95_ms;Max=$last.max_ms;IncludesConnectionHealthCheck=$true;Window='After restart through final shutdown; pre-crash in-memory samples are discarded'}|ConvertTo-Json
"""))
            report["Cleanup"] = operation("Cleanup")
        finally:
            report["FinishedUtc"] = dt.datetime.now(dt.timezone.utc).isoformat()
            save_report()
    print(json.dumps({k: v for k, v in report.items() if k not in ("StateBeforeCleanup", "Failures")}), flush=True)


if __name__ == "__main__":
    main()
