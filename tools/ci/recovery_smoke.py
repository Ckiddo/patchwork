"""Transport recovery, actual server crash and heartbeat deadline against an isolated DB."""
import json
import time
import uuid
from contextlib import ExitStack
from websockets.sync.client import connect
from websockets.exceptions import ConnectionClosed
from friends_smoke import Client, integer, expect_room
from websocket_smoke import field, decode


def call(client, tag, body):
    request_id = str(uuid.uuid4()).encode()
    client.socket.send(integer(1, 1) + field(2, request_id) + field(tag, body))
    while True:
        value = client.receive()
        if value.get(2) == request_id:
            return value


def synchronize(client, game):
    # A lifecycle transition can race the ACK; obtain a fresh committed cursor.
    for _ in range(5):
        resumed = decode(call(client, 15, field(1, game))[18])
        reply = call(client, 16, field(1, game) + integer(2, resumed.get(2, 0)) +
                     integer(3, resumed.get(3, 0)) + field(4, resumed[4]))
        if 11 in reply:
            return resumed
        assert decode(reply[10])[1] == 10
    raise AssertionError('recovery did not converge')


def check(base, request, credentials, restart):
    url = base.replace('http://', 'ws://', 1) + '/api/ws'
    origin = 'https://ckiddo.github.io'
    with ExitStack() as stack:
        identities = []
        clients = []
        for _ in range(2):
            status, _, value = request('/api/auth/create', 'POST')
            assert status == 200
            identities.append(value)
            credentials.extend([value['jwt'], value['refresh_token']])
            clients.append(Client(stack.enter_context(connect(url, origin=origin)), value['jwt']))
        a, b = clients
        room = expect_room(a.lobby(10, field(1, b'casual') + field(2, b'v1')))
        room_id = room[1].decode()
        room = expect_room(b.lobby(11, field(1, room[8])))
        room = expect_room(a.lobby(13, integer(1, 1), room_id, room[2]))
        room = expect_room(b.lobby(13, integer(1, 1), room_id, room[2]))
        start_version, start_id = room[2], str(uuid.uuid4())
        initial = expect_room(a.lobby(14, room=room_id, version=start_version, request_id=start_id))
        game = initial[7]
        assert decode(call(a, 14, field(1, game) + field(10, b''))[10])[1] == 20
        original = synchronize(a, game)
        synchronize(b, game)
        original_state = json.loads(decode(original[5])[5])
        restart()  # Kill the live process without its shutdown path, then relaunch the same config.
        restored = []
        for identity in identities:
            restored.append(Client(stack.enter_context(connect(url, origin=origin)), identity['jwt']))
        a, b = restored
        current = expect_room(a.lobby(16))
        assert current[1] == initial[1] and current[7] == game
        replay = expect_room(a.lobby(14, room=room_id, version=start_version, request_id=start_id))
        assert replay == initial, 'crash recovery duplicated the start command'
        recovered = synchronize(a, game)
        synchronize(b, game)
        assert recovered.get(2, 0) >= original.get(2, 0)
        assert json.loads(decode(recovered[5])[5]) == original_state, 'restart changed seats or committed state'
        assert decode(call(a, 14, field(1, game) + field(10, b''))[10])[1] == 6

        # Takeover must fence the previous socket, including its delayed cleanup.
        newest = Client(stack.enter_context(connect(url, origin=origin)), identities[0]['jwt'])
        synchronize(newest, game)
        try:
            call(a, 11, integer(1, 1))
            raise AssertionError('replaced socket accepted another request')
        except ConnectionClosed:
            pass
        assert 15 in call(newest, 11, integer(1, 1))

        # Oversize frames and bursts close only the offender.
        for payload in [b'x' * 17000, None]:
            offender = Client(stack.enter_context(connect(url, origin=origin)), identities[0]['jwt'])
            try:
                if payload:
                    offender.socket.send(payload)
                else:
                    for _ in range(90):
                        offender.socket.send(integer(1, 1) + field(2, b'rate') + field(11, integer(1, 1)))
                while True:
                    offender.receive()
            except ConnectionClosed:
                pass
            assert 15 in call(b, 11, integer(1, 1)), 'offender blocked peer'
    print('Recovery transport passed: actual process crash, fixed seats, original receipt, resume/ACK, takeover, size and rate limits.', flush=True)


def heartbeat_check(base, request, credentials):
    url = base.replace('http://', 'ws://', 1) + '/api/ws'
    origin = 'https://ckiddo.github.io'
    with ExitStack() as stack:
        clients = []
        for _ in range(2):
            _, _, identity = request('/api/auth/create', 'POST')
            credentials.extend([identity['jwt'], identity['refresh_token']])
            clients.append(Client(stack.enter_context(connect(url, origin=origin)), identity['jwt']))
        active, silent = clients
        room = expect_room(active.lobby(10, field(1, b'casual') + field(2, b'v1')))
        room = expect_room(silent.lobby(11, field(1, room[8])))
        deadline = time.monotonic() + 50
        pushes = 0
        while time.monotonic() < deadline:
            current = expect_room(active.lobby(16, room=room[1].decode()))
            expect_room(active.lobby(13, integer(1, 1), room[1].decode(), current[2]))
            try:
                silent.receive()
                pushes += 1
            except ConnectionClosed:
                assert pushes > 30, 'test did not sustain outgoing traffic'
                assert 15 in call(active, 11, integer(1, 1))
                print('Heartbeat passed: 45-second application-input deadline despite continuous outgoing room pushes.', flush=True)
                return
            time.sleep(1)
        raise AssertionError('outgoing traffic kept silent client alive')
