"""Local-only T40 control and audit. No supplied database URL or credentials accepted."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import tomllib
import uuid

ROOT = Path(__file__).resolve().parents[2]
ART = ROOT / 'artifacts'


def context():
    meta = json.loads((ART / 't40-preview.json').read_text())
    work = Path(meta['cluster']).resolve()
    assert work.parent == ART.resolve() and work.name.startswith('postgres-test-')
    config = tomllib.loads((work / 'admin.toml').read_text())['database']
    assert config['host'] == '127.0.0.1' and config['name'].startswith('patchwork_test_')
    key = (work / config['password_file']).resolve()
    assert key.parent == work
    env = dict(os.environ, PGHOST='127.0.0.1', PGPORT=str(config['port']), PGUSER=config['user'], PGDATABASE=config['name'], PGPASSWORD=key.read_text().strip(), PGCONNECT_TIMEOUT='5')
    return work, env


def sql(env, query):
    p = subprocess.run([r'D:\Tools\PostgreSQL\18.6\pgsql\bin\psql.exe', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1'], input=query, text=True, capture_output=True, env=env, timeout=15)
    assert p.returncode == 0, 'T40 isolated SQL failed (private output suppressed)'
    return p.stdout.strip()


def oracle(state):
    p = subprocess.run([str(ROOT / 'target/debug/examples/t40_oracle.exe')], input=json.dumps({'snapshot':state}), text=True, capture_output=True, timeout=30)
    assert p.returncode == 0, 'Core rejected T40 snapshot: ' + p.stderr[:300]
    return json.loads(p.stdout)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('command', choices=['pause','resume','restart','drop','crash','inspect','fixture','expiry'])
    parser.add_argument('--game', type=uuid.UUID)
    parser.add_argument('--kind')
    args = parser.parse_args()
    work, env = context()
    if args.command == 'pause':
        (work / 'backend.pause').write_text('pause')
        for _ in range(100):
            with socket.socket() as s:
                if s.connect_ex(('127.0.0.1',8000)) != 0: break
            time.sleep(.05)
        else: raise AssertionError('backend did not stop')
    elif args.command == 'resume':
        (work / 'backend.pause').unlink(missing_ok=True)
    elif args.command == 'restart':
        (ART / 'browser-preview.restart').write_text('manual')
    elif args.command in ('drop','crash'):
        (work / 'commit-fault.arm').write_text(args.command)
    elif args.command == 'expiry':
        import urllib.request
        from websocket_smoke import expiry_check
        base = 'http://127.0.0.1:8000'
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        def request(path, method):
            with opener.open(urllib.request.Request(base + path, method=method), timeout=5) as response:
                return response.status, response.headers, json.load(response)
        config = tomllib.loads((work / 'browser.toml').read_text())
        key = (work / config['jwt_secret_file']).resolve()
        assert key.parent == work
        expiry_check(base, key.read_text().strip(), request, [], 'http://127.0.0.1:8082')
    else:
        assert args.game is not None
        game = str(args.game)
        source = json.loads(sql(env, f"SELECT json_build_object('snapshot',snapshot,'version',state_version,'room_id',room_id,'player0',player0,'player1',player1) FROM patchwork.games WHERE game_id='{game}'"))
        if args.command == 'fixture':
            assert (work / 'backend.pause').exists()
            with socket.socket() as s: assert s.connect_ex(('127.0.0.1',8000)) != 0
            from t40_fixtures import make
            state = make(source['snapshot'], args.kind)
            oracle(state)
            assert state['game_id'] == game and [p['user_id'] for p in state['players']] == [source['player0'], source['player1']]
            assert source['snapshot']['result'] is None and state['result'] is None
            body = json.dumps(state, separators=(',', ':')).replace("'", "''")
            version = source['version'] + 1
            # A labelled fixture transition preserves the event cursor; no command receipt is fabricated.
            sql(env, f"""BEGIN;
UPDATE patchwork.games SET snapshot='{body}'::jsonb,phase='playing',state_version={version},event_seq={version},updated_at=now() WHERE game_id='{game}' AND state_version={source['version']};
INSERT INTO patchwork.game_events(game_id,seq,state_version,user_id,payload) VALUES('{game}',{version},{version},'{source['player0']}',jsonb_build_object('kind','game_transition_v1','game_id','{game}','source','t40_fixture','rules_version','patchwork_custom_v1','phase','playing','events','[]'::jsonb,'state','{body}'::jsonb));
UPDATE patchwork.rooms SET version=version+1 WHERE room_id='{source['room_id']}';
COMMIT;""")
            (ART / f't40-fixture-{args.kind}.json').write_text(json.dumps(state, indent=2))
        audit = json.loads(sql(env, f"""BEGIN READ ONLY; SELECT json_build_object('snapshot',g.snapshot,'version',g.state_version,'seq',g.event_seq,'events',(SELECT count(*) FROM patchwork.game_events WHERE game_id=g.game_id),'receipts',(SELECT count(*) FROM patchwork.command_receipts WHERE game_id=g.game_id),'results',(SELECT count(*) FROM patchwork.game_results WHERE game_id=g.game_id),'bonus_events',(SELECT count(*) FROM patchwork.game_events e CROSS JOIN LATERAL jsonb_array_elements(COALESCE(e.payload->'events','[]'::jsonb)) x WHERE e.game_id=g.game_id AND x->>'kind'='bonus_awarded'),'fixture_events',(SELECT count(*) FROM patchwork.game_events WHERE game_id=g.game_id AND payload->>'source'='t40_fixture')) FROM patchwork.games g WHERE g.game_id='{game}'; COMMIT;"""))
        state = audit.pop('snapshot')
        info = oracle(state)
        (ART / 't40-current-snapshot.json').write_text(json.dumps(state, indent=2))
        (ART / f't40-game-{game}-v{audit["version"]}.json').write_text(json.dumps(state, indent=2))
        report = {'game':game,**audit,**info}
        assert audit['version'] == audit['seq'] == audit['events']
        assert audit['results'] == int(info['result'] is not None)
        with (ART / 't40-database-audit.jsonl').open('a') as out: out.write(json.dumps(report)+'\n')
        print(json.dumps(report))
    print('T40 ' + args.command + ': PASS')


if __name__ == '__main__': main()
