"""Real custom-rule actions and recovery through two authenticated WebSocket clients."""
import json
import time
import uuid
from contextlib import ExitStack
from websockets.sync.client import connect
from friends_smoke import Client, integer, expect_room
from recovery_smoke import call, synchronize
from websocket_smoke import field, decode


def send_action(client, game, version, tag, payload=b'', request_id=None):
    request_id = (request_id or str(uuid.uuid4())).encode()
    body = field(1, game) + integer(2, version) + field(tag, payload)
    client.socket.send(integer(1, 1) + field(2, request_id) + field(14, body))
    while True:
        reply = client.receive()
        if reply.get(2) == request_id:
            return reply


def state(snapshot):
    value = json.loads(snapshot[5])
    assert value['kind'] == 'patchwork_game_v1'
    return value


def synchronized_pair(clients, game):
    for client in clients:
        synchronize(client, game)
    deadline = time.monotonic() + 10
    latest = None
    while time.monotonic() < deadline:
        for client in clients:
            assert 15 in call(client, 11, integer(1, 1))
            for snapshot in client.games:
                if snapshot[1] == game and (latest is None or snapshot.get(2, 0) > latest.get(2, 0)):
                    latest = snapshot
            client.games.clear()
        if latest is not None and state(latest)['lifecycle'] == 'running':
            return latest
        time.sleep(0.1)
    raise AssertionError('custom game did not resume')


def committed_pair(clients, game, version):
    copies = []
    for client in clients:
        while True:
            snapshot = client.game()
            if snapshot[1] == game and snapshot.get(2, 0) >= version:
                assert snapshot.get(2, 0) == version, 'unexpected transition during active game'
                copies.append(snapshot)
                break
    assert copies[0] == copies[1], 'players received different committed snapshots'
    return copies[0]


def actor_of(value):
    pending = value['action']['pending_specials']
    if pending:
        return pending[0]['owner']
    times = [p['time_position'] for p in value['players']]
    if times[0] != times[1]:
        return int(times[0] > times[1])
    last = value['action']['last_normal_actor']
    return value['first_player_seat'] if last is None else 1 - last


def check(base, request, credentials, restart):
    url = base.replace('http://', 'ws://', 1) + '/api/ws'
    with ExitStack() as stack:
        identities, clients = [], []
        for _ in range(2):
            status, _, identity = request('/api/auth/create', 'POST')
            assert status == 200
            identities.append(identity)
            credentials.extend([identity['jwt'], identity['refresh_token']])
            clients.append(Client(stack.enter_context(connect(url, origin='https://ckiddo.github.io')), identity['jwt']))
        a, b = clients
        room = expect_room(a.lobby(10, field(1, b'casual') + field(2, b'patchwork_custom_v1')))
        room_id = room[1].decode()
        room = expect_room(b.lobby(11, field(1, room[8])))
        room = expect_room(a.lobby(13, integer(1, 1), room_id, room[2]))
        room = expect_room(b.lobby(13, integer(1, 1), room_id, room[2]))
        start_version, start_id = room[2], str(uuid.uuid4())
        initial = expect_room(a.lobby(14, room=room_id, version=start_version, request_id=start_id))
        game = initial[7]
        snapshot = synchronized_pair(clients, game)
        initial_order = state(snapshot)['supply']['initial_order']
        assert len(initial_order) == len(set(initial_order)) == 33
        assert expect_room(a.lobby(14, room=room_id, version=start_version, request_id=start_id)) == initial
        # Replaying start can publish the current snapshot; discard those older copies below.
        value = state(snapshot)
        owner = actor_of(value)
        purchase = field(1, b'10') + field(2, b'')  # Valid presence, anchor (0, 0).
        purchase_version, purchase_id = snapshot.get(2, 0), str(uuid.uuid4())
        reply = send_action(clients[owner], game, purchase_version, 11, purchase, purchase_id)
        assert 11 in reply, f'purchase rejected: {decode(reply.get(10, b""))}'
        assert send_action(clients[owner], game, purchase_version, 11, purchase, purchase_id) == reply
        version = decode(reply[11]).get(2, 0)
        snapshot = committed_pair(clients, game, version)
        assert state(snapshot)['players'][owner]['buttons'] == 3

        restarted = False
        for step in range(112):
            value = state(snapshot)
            assert value['supply']['initial_order'] == initial_order
            if value['result'] is not None:
                break
            pending = value['action']['pending_specials']
            if pending and not restarted:
                saved = value
                restart()  # Actual forced process exit with an unplaced special patch.
                clients = [Client(stack.enter_context(connect(url, origin='https://ckiddo.github.io')), i['jwt']) for i in identities]
                snapshot = synchronized_pair(clients, game)
                assert state(snapshot) == saved, 'restart lost the purchased patch, resources or pending queue'
                restarted = True
                value = state(snapshot)
            actor = actor_of(value)
            tag, body = 10, b''
            if value['action']['pending_specials']:
                cells = value['players'][actor]['board']['cells']
                x, y = next((x, y) for y in range(9) for x in range(9) if cells[y][x] is None)
                # BoardPosition uses sint32, whose nonnegative wire value is doubled.
                tag, body = 12, integer(1, x << 1) + integer(2, y << 1)
            version, command_id = snapshot.get(2, 0), str(uuid.uuid4())
            reply = send_action(clients[actor], game, version, tag, body, command_id)
            assert 11 in reply, f'action rejected: {decode(reply.get(10, b""))}'
            snapshot = committed_pair(clients, game, decode(reply[11]).get(2, 0))
            if state(snapshot)['result'] is not None:
                assert send_action(clients[actor], game, version, tag, body, command_id) == reply
            time.sleep(0.05)  # Keep deliberate actions below the production input-rate limit.
        final = state(snapshot)
        assert restarted and final['lifecycle'] == 'finished'
        assert [p['time_position'] for p in final['players']] == [53, 53]
        assert final['result']['reason']['kind'] == 'scored'
        assert [s['total'] for s in final['result']['scores']] == [p['buttons'] for p in final['players']]
        restart()
        clients = [Client(stack.enter_context(connect(url, origin='https://ckiddo.github.io')), i['jwt']) for i in identities]
        for client in clients:
            resumed = synchronize(client, game)
            assert state(decode(resumed[5])) == final, 'finished result changed after restart'
    print('Custom gameplay passed: two real WebSockets, purchase retry, special-queue process crash, full natural game, identical snapshots, persisted final result after second restart.', flush=True)
