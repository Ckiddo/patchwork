"""Opt-in local acceptance relay; never used by the production server.

Observe PostgreSQL CommandComplete after a game command commits. Only the
command name is inspected; SQL parameters, authentication and payloads aren't logged.
"""
import json
from pathlib import Path
import socket
import struct
import threading
import time


def exact(sock, size):
    data = bytearray()
    while len(data) < size:
        part = sock.recv(size - len(data))
        if not part:
            raise EOFError
        data.extend(part)
    return bytes(data)


class FaultProxy:
    def __init__(self, upstream, work, restart):
        self.upstream, self.work, self.restart = upstream, Path(work), Path(restart)
        self.arm = self.work / 'commit-fault.arm'
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.sockets = set()
        self.listener = socket.create_server(('127.0.0.1', 0))
        self.port = self.listener.getsockname()[1]
        self.listener.settimeout(0.2)
        threading.Thread(target=self.accept, daemon=True).start()

    def accept(self):
        while not self.stop.is_set():
            try:
                client, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            threading.Thread(target=self.relay, args=(client,), daemon=True).start()

    def relay(self, client):
        server = None
        try:
            server = socket.create_connection(('127.0.0.1', self.upstream), timeout=5)
            server.settimeout(None)
            with self.lock:
                self.sockets.update([client, server])
            length = exact(client, 4)
            n, = struct.unpack('!I', length)
            assert 8 <= n <= 65536
            server.sendall(length + exact(client, n - 4))
            transaction = {'game': False}
            statements = {}

            def upload():
                try:
                    while not self.stop.is_set():
                        tag, length = exact(client, 1), exact(client, 4)
                        n, = struct.unpack('!I', length)
                        assert 4 <= n <= 1_048_576
                        payload = exact(client, n - 4)
                        sql = b''
                        if tag == b'P':
                            name, sql, _ = payload.split(b'\0', 2)
                            statements[name] = sql.lower()
                        elif tag == b'B':
                            _, name, _ = payload.split(b'\0', 2)
                            sql = statements.get(name, b'')
                        elif tag == b'Q':
                            sql = payload.lower()
                        if b'insert into patchwork.command_receipts' in sql:
                            transaction['game'] = True
                        server.sendall(tag + length + payload)
                except (OSError, EOFError, AssertionError, ValueError):
                    try:
                        server.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass

            threading.Thread(target=upload, daemon=True).start()
            while not self.stop.is_set():
                tag, length = exact(server, 1), exact(server, 4)
                n, = struct.unpack('!I', length)
                assert 4 <= n <= 1_048_576
                payload = exact(server, n - 4)
                if tag == b'C' and payload in (b'COMMIT\0', b'ROLLBACK\0'):
                    mode = None
                    with self.lock:
                        if payload == b'COMMIT\0' and transaction['game'] and self.arm.exists():
                            mode = self.arm.read_text().strip()
                            assert mode in ('drop', 'crash')
                            self.arm.unlink()
                            with (self.work.parent / 't40-faults.jsonl').open('a') as out:
                                out.write(json.dumps({'event': 'game_commit_response_withheld', 'mode': mode, 'at': time.time()}) + '\n')
                        transaction['game'] = False
                    if mode == 'crash':
                        self.restart.write_text('commit-crash')
                        # Do not let the backend recover/ACK before its process is killed.
                        while not self.stop.wait(0.05):
                            if not self.restart.exists():
                                time.sleep(0.5)
                                break
                        return
                    if mode == 'drop':
                        return
                client.sendall(tag + length + payload)
        except (OSError, EOFError, AssertionError, ValueError):
            pass
        finally:
            for sock in (client, server):
                if sock is not None:
                    with self.lock:
                        self.sockets.discard(sock)
                    try:
                        sock.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                    sock.close()

    def close(self):
        self.stop.set()
        self.listener.close()
        with self.lock:
            for sock in self.sockets:
                try:
                    sock.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
