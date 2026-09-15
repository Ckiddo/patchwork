"""Real WebSocket friend-room flow against the isolated local backend."""
import json
import uuid
from contextlib import ExitStack
from websockets.sync.client import connect
from websocket_smoke import field, varint, decode


def integer(number, value):
    return varint(number << 3) + varint(value)


class Client:
    def __init__(self, socket, token):
        self.socket = socket
        self.games = []
        self.socket.send(integer(1, 1) + field(2, b'auth') + field(10, field(1, token.encode())))
        assert 16 in decode(self.socket.recv(timeout=5)), 'authentication failed'

    def receive(self):
        value = decode(self.socket.recv(timeout=8))
        if 14 in value:
            self.games.append(decode(value[14]))
        return value

    def lobby(self, tag, payload=b'', room='', version=0, request_id=None):
        request_id = request_id or str(uuid.uuid4())
        body = field(1, room.encode()) + integer(2, version) + field(tag, payload)
        self.socket.send(integer(1, 1) + field(2, request_id.encode()) + field(12, body))
        while True:
            value = self.receive()
            if value.get(2) == request_id.encode():
                return value

    def game(self):
        while not self.games:
            self.receive()
        return self.games.pop(0)


def expect_room(response):
    assert 12 in response, f'expected room, error code {decode(response.get(10, b"")).get(1)}'
    return decode(response[12])


def expect_error(response, code):
    actual = decode(response.get(10, b'')).get(1)
    assert actual == code, f'wrong room error: expected {code}, got {actual}'


def check(base, request, credentials):
    url = base.replace('http://', 'ws://', 1) + '/api/ws'
    password = 'isolated-room-smoke-password'
    credentials.append(password)
    with ExitStack() as stack:
        clients = []
        users = []
        for _ in range(4):
            status, _, identity = request('/api/auth/create', 'POST')
            assert status == 200
            credentials.extend([identity['jwt'], identity['refresh_token']])
            users.append(identity['identity']['user_id'])
            socket = stack.enter_context(connect(url, origin='https://ckiddo.github.io', open_timeout=5))
            clients.append(Client(socket, identity['jwt']))
        host, guest, third, fourth = clients
        assert 17 in host.lobby(15, integer(2, 10)), 'room list unavailable'
        create = field(1, b'casual') + field(2, b'v1') + field(3, password.encode())
        room = expect_room(host.lobby(10, create))
        room_id, code = room[1].decode(), room[8]
        assert room.get(10) == 1, 'password marker missing'
        expect_error(host.lobby(14, room=room_id), 9)
        expect_error(guest.lobby(11, field(1, code) + field(2, b'wrong')), 19)
        joined = expect_room(guest.lobby(11, field(1, code) + field(2, password.encode())))
        assert joined[1] == room[1]
        version = joined[2]
        version = expect_room(host.lobby(13, integer(1, 1), room_id, version))[2]
        version = expect_room(guest.lobby(13, integer(1, 1), room_id, version))[2]
        start_id = str(uuid.uuid4())
        started = expect_room(host.lobby(14, room=room_id, version=version, request_id=start_id))
        repeated = expect_room(host.lobby(14, room=room_id, version=version, request_id=start_id))
        assert started == repeated and started[3] == 3
        host_game, guest_game = host.game(), guest.game()
        assert host_game == guest_game and host_game[1] == started[7], 'initial snapshots differ'
        state = json.loads(host_game[5])
        assert [p['user_id'] for p in state['players']] == users[:2]
        assert state['rules_implemented'] is False and state['first_player_seat'] in (0, 1)
        # The first recovery tick can bump the room version while password verification
        # is running. Retry only that rejected version check, then still require the
        # actual no-join result. Do not treat VERSION_CONFLICT itself as a passing test.
        for _ in range(3):
            rejected = third.lobby(11, field(1, code) + field(2, password.encode()))
            if decode(rejected.get(10, b'')).get(1) != 10:
                expect_error(rejected, 8)
                break
        else:
            raise AssertionError('room version did not stabilize for the rejected join')
        expect_error(third.lobby(16, room=room_id), 15)
        current = expect_room(host.lobby(16, room=room_id))
        expect_error(host.lobby(14, room=room_id, version=current[2]), 8)
        other = expect_room(third.lobby(10, field(1, b'casual') + field(2, b'v1')))
        other_id = other[1].decode()
        other = expect_room(fourth.lobby(11, field(1, other[8])))
        other = expect_room(third.lobby(12, room=other_id, version=other[2]))
        assert other[5].decode() == users[3], 'owner transfer failed'
        other = expect_room(fourth.lobby(12, room=other_id, version=other[2]))
        assert other[3] == 5 and not other.get(5), 'empty room not closed'
    print('Friend-room WebSocket smoke passed: passwords, join, ready, identical game, idempotent start, transfer and close.')
