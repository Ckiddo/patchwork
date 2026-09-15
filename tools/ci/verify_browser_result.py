"""Read-only verification of a game played through the local browser preview."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tomllib
import uuid

parser = argparse.ArgumentParser()
parser.add_argument("--cluster", type=Path, required=True)
parser.add_argument("--game", type=uuid.UUID, required=True)
parser.add_argument("--scores", type=int, nargs=2, required=True)
parser.add_argument("--psql", type=Path, default=Path(r"D:\Tools\PostgreSQL\18.6\pgsql\bin\psql.exe"))
args = parser.parse_args()
root = Path(__file__).resolve().parents[2]
cluster = args.cluster.resolve()
assert cluster.parent == (root / "artifacts").resolve() and cluster.name.startswith("postgres-test-")
db = tomllib.loads((cluster / "app.toml").read_text())["database"]
assert db["host"] == "127.0.0.1" and db["name"].startswith("patchwork_test_")
key = (cluster / db["password_file"]).resolve()
assert key.parent == cluster
env = dict(os.environ, PGHOST=db["host"], PGPORT=str(db["port"]), PGUSER=db["user"],
           PGDATABASE=db["name"], PGPASSWORD=key.read_text().strip(), PGCONNECT_TIMEOUT="5")
query = f"""BEGIN READ ONLY;
SELECT json_build_object('phase',g.phase,'version',g.state_version,'event_seq',g.event_seq,
 'kind',g.snapshot->>'kind',
 'events',(SELECT count(*) FROM patchwork.game_events WHERE game_id=g.game_id),
 'receipts',(SELECT count(*) FROM patchwork.command_receipts WHERE game_id=g.game_id),
 'results',(SELECT count(*) FROM patchwork.game_results WHERE game_id=g.game_id),
 'score0',r.score0,'score1',r.score1,'winner',r.winner_seat,'reason',r.reason)
FROM patchwork.games g JOIN patchwork.game_results r USING(game_id)
WHERE g.game_id='{args.game}';
COMMIT;
"""
result = subprocess.run([str(args.psql), "-X", "-qAt", "-v", "ON_ERROR_STOP=1"],
                        input=query, text=True, capture_output=True, env=env, timeout=15)
assert result.returncode == 0, "Local read-only database verification failed"
value = json.loads(result.stdout)
assert value["phase"] == "finished" and value["reason"] == "completed"
assert value["kind"] == "patchwork_game_v1" and value["results"] == 1
assert [value["score0"], value["score1"]] == args.scores
assert value["version"] == value["event_seq"] == value["events"]
print(json.dumps(value, sort_keys=True))
print("Browser game persisted with one result and contiguous committed versions: PASS")
