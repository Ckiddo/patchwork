"""Own an isolated PostgreSQL test cluster and run the SQLx integration suite.

No DATABASE_URL is accepted. External mode is ONLY the disposable CI service.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import tempfile
import sys
import time

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--bin-dir', type=Path)
    parser.add_argument('--ci-service', action='store_true')
    parser.add_argument('--preview-dir', type=Path, help='Temporarily host a local API and frontend for manual browser checks')
    parser.add_argument('--preview-port', type=int, default=8080)
    parser.add_argument('--managed-preview', action='store_true', help='The local launcher owns the stop signal; do not erase a stop requested during initialization')
    parser.add_argument('--acceptance', action='store_true', help='Opt-in local T40 pause and COMMIT-fault controls')
    parser.add_argument('--test-binary', type=Path, help='Run an already-built integration test executable without relinking the active preview server')
    parser.add_argument('--skip-process-smoke', action='store_true', help='Local database-only run while another backend owns the single-instance lock')
    args = parser.parse_args()
    if args.acceptance and not args.preview_dir:
        parser.error('--acceptance requires --preview-dir')
    if args.managed_preview and not args.preview_dir:
        parser.error('--managed-preview requires --preview-dir')
    if args.ci_service and (args.test_binary or args.skip_process_smoke):
        parser.error('CI must build its tests and run the process smoke test')
    if args.preview_dir and (args.test_binary or args.skip_process_smoke):
        parser.error('test execution options cannot be used in preview mode')
    if args.test_binary:
        args.test_binary = args.test_binary.resolve()
        if not args.test_binary.is_relative_to(ROOT) or not args.test_binary.is_file():
            parser.error('--test-binary must be an existing executable in this workspace')
    artifacts = ROOT / 'artifacts'
    artifacts.mkdir(exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix='postgres-test-', dir=artifacts)).resolve()
    assert work.is_relative_to(artifacts.resolve())
    suffix = secrets.token_hex(6)
    database = 'patchwork_test_' + suffix
    passwords = {role: secrets.token_hex(32) for role in ['admin', 'migrate', 'app']}
    bindir = args.bin_dir

    def exe(name):
        return str(bindir / (name + ('.exe' if os.name == 'nt' else ''))) if bindir else name

    def run(cmd, **kwargs):
        if Path(str(cmd[0])).stem == 'pg_ctl':
            # A Windows server child can inherit PIPE handles past pg_ctl's exit.
            with (work / 'pg_ctl.log').open('a') as log:
                result = subprocess.run(cmd, cwd=ROOT, stdout=log, stderr=log, **kwargs)
            if result.returncode:
                raise RuntimeError('pg_ctl failed')
            return result
        result = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, **kwargs)
        if result.returncode:
            # Never print SQL/server output: constraint errors can echo private row values.
            raise RuntimeError(f'{Path(str(cmd[0])).name} failed (exit {result.returncode})')
        return result

    started = False
    safe_cleanup = True
    try:
        if args.ci_service:
            assert os.environ.get('CI') == 'true', 'external mode requires disposable CI environment'
            port = 5432
            admin = 'patchwork_ci'
            admin_password = os.environ['PGPASSWORD']
            base_db = 'patchwork_ci'
        else:
            if bindir is None:
                raise RuntimeError('--bin-dir pointing to PostgreSQL binaries is required')
            with socket.socket() as sock:
                sock.bind(('127.0.0.1', 0))
                port = sock.getsockname()[1]
            admin = 'patchwork_test_admin'
            admin_password = passwords['admin']
            base_db = 'postgres'
            pwfile = work / 'admin.key'
            pwfile.write_text(admin_password, encoding='utf-8')
            run([exe('initdb'), '-D', str(work / 'data'), '-U', admin,
                 '--pwfile=' + str(pwfile), '--auth=scram-sha-256', '--encoding=UTF8', '--locale=C', '--data-checksums'])
            with (work / 'data/postgresql.conf').open('a') as f:
                f.write(f"\nlisten_addresses='127.0.0.1'\nport={port}\nmax_connections=32\nshared_buffers='32MB'\nlog_statement='none'\nlog_min_error_statement='panic'\n")
            started = True
            run([exe('pg_ctl'), '-D', str(work / 'data'), '-l', str(work / 'postgres.log'), '-w', 'start'], timeout=30)
        env = dict(os.environ, PGHOST='127.0.0.1', PGPORT=str(port), PGUSER=admin,
                   PGPASSWORD=admin_password, PGDATABASE=base_db, PGCONNECT_TIMEOUT='5')

        def sql(source, db=base_db):
            return run([exe('psql'), '-X', '-v', 'ON_ERROR_STOP=1', '-d', db], input=source, env=env).stdout

        # The external CI service is provisioned by this workflow, never a project DB.
        if args.ci_service:
            assert 'patchwork_ci' in sql('SELECT current_database();')
        bootstrap_env = dict(env, PATCHWORK_MIGRATE_PASSWORD=passwords['migrate'],
                             PATCHWORK_APP_PASSWORD=passwords['app'])
        run([exe('psql'), '-X', '-v', 'ON_ERROR_STOP=1', '-v', 'db_name=' + database,
             '-f', str(ROOT / 'tools/db/bootstrap.sql')], env=bootstrap_env)
        configs = {}
        for kind, user, password in [('app', 'patchwork_app', passwords['app']),
                                     ('migrate', 'patchwork_migrate', passwords['migrate']),
                                     ('admin', admin, admin_password)]:
            (work / f'{kind}.key').write_text(password, encoding='utf-8')
            config = work / f'{kind}.toml'
            config.write_text(f'''listen = "127.0.0.1:0"
jwt_secret_file = "app.key"
allowed_origins = ["https://ckiddo.github.io"]
workers = 2
shutdown_timeout_secs = 3
log_level = "info"
[database]
host = "127.0.0.1"
port = {port}
name = "{database}"
user = "{user}"
password_file = "{kind}.key"
''', encoding='utf-8')
            configs[kind] = str(config)
        binary = ROOT / 'target/debug' / ('patchwork-migrate.exe' if os.name == 'nt' else 'patchwork-migrate')
        for _ in range(2):
            run([str(binary), '--config', configs['migrate']], timeout=30)
        if args.preview_dir:
            assert 1024 <= args.preview_port <= 65535
            preview = args.preview_dir.resolve()
            assert not args.ci_service and preview.is_relative_to(artifacts.resolve())
            assert (preview / 'index.html').is_file()
            stop = artifacts / 'browser-preview.stop'
            restart = artifacts / 'browser-preview.restart'
            if restart.exists():
                restart.unlink()
            if stop.exists() and not args.managed_preview:
                stop.unlink()
            app_config = work / 'browser.toml'
            app_config.write_text(Path(configs['app']).read_text().replace(
                'listen = "127.0.0.1:0"', 'listen = "127.0.0.1:8000"').replace(
                'allowed_origins = ["https://ckiddo.github.io"]',
                f'allowed_origins = ["http://127.0.0.1:{args.preview_port}", "http://localhost:{args.preview_port}"]'), encoding='utf-8')
            server = ROOT / 'target/debug' / ('patchwork-server.exe' if os.name == 'nt' else 'patchwork-server')
            fault_proxy = None
            if args.acceptance:
                from preview_faults import FaultProxy
                fault_proxy = FaultProxy(port, work, restart)
                app_config.write_text(app_config.read_text().replace(f'port = {port}\n', f'port = {fault_proxy.port}\n'), encoding='utf-8')
                (artifacts / 't40-preview.json').write_text(json.dumps({'cluster': str(work), 'runner_pid': os.getpid(), 'database_port': port, 'proxy_port': fault_proxy.port}), encoding='utf-8')
            children = []
            with (work / 'browser-backend.log').open('w') as backend_log, (work / 'browser-http.log').open('w') as http_log:
                try:
                    children.append(subprocess.Popen([str(server), '--config', str(app_config)], cwd=ROOT, stdout=backend_log, stderr=backend_log))
                    children.append(subprocess.Popen([sys.executable, '-m', 'http.server', str(args.preview_port), '--bind', '127.0.0.1', '--directory', str(preview)], cwd=ROOT, stdout=http_log, stderr=http_log))
                    print(f'Preview: http://127.0.0.1:{args.preview_port} and http://localhost:{args.preview_port}; API on loopback port 8000.', flush=True)
                    print('Create artifacts/browser-preview.stop to stop and clean the temporary cluster.', flush=True)
                    paused = False
                    while not stop.exists():
                        pause_requested = args.acceptance and (work / 'backend.pause').exists()
                        if pause_requested and not paused:
                            children[0].kill()
                            children[0].wait(timeout=10)
                            paused = True
                            print('Acceptance backend paused; database and frontend retained.', flush=True)
                        if paused and not pause_requested:
                            children[0] = subprocess.Popen([str(server), '--config', str(app_config)], cwd=ROOT, stdout=backend_log, stderr=backend_log)
                            paused = False
                            print('Acceptance backend resumed.', flush=True)
                        if restart.exists():
                            children[0].kill()
                            children[0].wait(timeout=10)
                            restart.unlink()
                            time.sleep(2)
                            children[0] = subprocess.Popen([str(server), '--config', str(app_config)], cwd=ROOT, stdout=backend_log, stderr=backend_log)
                            print('Preview backend restarted with the same temporary database.', flush=True)
                        assert all(child.poll() is None for child in children[1:]) and (paused or children[0].poll() is None), 'preview child exited'
                        time.sleep(0.25)
                finally:
                    for child in children:
                        if child.poll() is None:
                            child.terminate()
                            child.wait(timeout=10)
                    if fault_proxy:
                        fault_proxy.close()
            return
        env.update(PATCHWORK_TEST_CONFIG=configs['app'], PATCHWORK_TEST_ADMIN_CONFIG=configs['admin'],
                   PATCHWORK_TEST_DATABASE=database)
        # Cargo test errors contain only disposable test data, not connection options.
        test_command = ([str(args.test_binary)] if args.test_binary else
                        ['cargo', 'test', '--locked', '-p', 'backend', '--test', 'postgres', '--'])
        result = subprocess.run(test_command + ['--ignored', '--test-threads=1'], cwd=ROOT, env=env)
        if result.returncode:
            raise RuntimeError('PostgreSQL integration tests failed')
        if not args.skip_process_smoke:
            subprocess.run([sys.executable, str(ROOT / 'tools/ci/smoke_backend.py'), '--database-config', configs['app']], cwd=ROOT, check=True)
        (artifacts / 'postgres-suite.json').write_text(json.dumps({
            'result': 'passed', 'database': 'isolated disposable test database',
            'migration_runs': 2, 'runtime_role': 'patchwork_app',
            'process_smoke': 'skipped (explicit local database-only run)' if args.skip_process_smoke else 'passed',
        }, indent=2) + '\n')
        print('PostgreSQL suite passed; disposable cluster cleanup follows.')
    finally:
        if started:
            try:
                run([exe('pg_ctl'), '-D', str(work / 'data'), '-m', 'immediate', '-w', 'stop'], timeout=30)
            except Exception:
                safe_cleanup = False
                raise
        # External CI service dies with the job; never issue a DROP against a supplied DB.
        if safe_cleanup:
            shutil.rmtree(work)
            if args.preview_dir:
                print('Preview stopped; disposable cluster removed.', flush=True)


if __name__ == '__main__':
    main()
