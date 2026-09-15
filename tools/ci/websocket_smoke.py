"""Minimal Protobuf wire probes against the temporary local server only."""
from websockets.sync.client import connect
from websockets.exceptions import ConnectionClosed, InvalidStatus
import base64
import hashlib
import hmac
import json
import time


def varint(value):
    out = bytearray()
    while value > 127:
        out.append((value & 127) | 128)
        value >>= 7
    out.append(value)
    return bytes(out)


def field(number, data):
    return varint((number << 3) | 2) + varint(len(data)) + data


def envelope(number, data):
    return b'\x08\x01' + field(2, b'smoke') + field(number, data)


def decode(data):
    assert isinstance(data, bytes), 'server did not use binary protobuf'
    pos = 0

    def integer():
        nonlocal pos
        value = shift = 0
        while True:
            assert pos < len(data) and shift < 70, 'invalid varint'
            b = data[pos]
            pos += 1
            value |= (b & 127) << shift
            if b < 128:
                return value
            shift += 7

    values = {}
    while pos < len(data):
        tag = integer()
        if tag & 7 == 0:
            value = integer()
        else:
            assert tag & 7 == 2, 'unexpected wire type'
            length = integer()
            value = data[pos:pos + length]
            assert len(value) == length
            pos += length
        values[tag >> 3] = value
    return values


def check(base, token, request):
    url = base.replace('http://', 'ws://', 1) + '/api/ws'
    origin = 'https://ckiddo.github.io'
    auth = envelope(10, field(1, token.encode()))
    ping = envelope(11, b'\x08\x01')
    for target, source in [(url, 'https://foreign.invalid'), (url + '?token=not-real-marker', origin)]:
        try:
            with connect(target, origin=source, open_timeout=5):
                raise AssertionError('invalid upgrade accepted')
        except InvalidStatus as e:
            assert e.response.status_code == 403
    with connect(url, origin=origin) as unauthenticated:
        unauthenticated.send(ping)
        assert decode(decode(unauthenticated.recv(timeout=5))[10])[1] == 5
    with connect(url, origin=origin) as silent:
        assert decode(decode(silent.recv(timeout=7))[10])[1] == 5
    with connect(url, origin=origin) as first:
        first.send(auth)
        first_generation = decode(decode(first.recv(timeout=5))[16])[2]
        with connect(url, origin=origin) as second:
            second.send(auth)
            second_generation = decode(decode(second.recv(timeout=5))[16])[2]
            assert second_generation > first_generation
            try:
                first.send(ping)
                first.recv(timeout=5)
                raise AssertionError('old connection remained active')
            except ConnectionClosed as error:
                assert error.rcvd.code == 1008, 'takeover must stop automatic reconnect'
            second.send(ping)
            assert decode(decode(second.recv(timeout=5))[15])[1] == 1
            status, _, _ = request('/api/auth/logout', 'POST', headers={'Authorization':'Bearer ' + token})
            assert status == 200
            try:
                second.recv(timeout=5)
                raise AssertionError('logout did not close the active connection')
            except ConnectionClosed:
                pass


def expiry_check(base, key, request, credentials, origin='https://ckiddo.github.io'):
    """Signed short-lived tokens exercise registry/socket expiry races on a real connection."""
    status, _, created = request('/api/auth/create', 'POST')
    assert status == 200
    credentials.extend([created['jwt'], created['refresh_token']])
    header, payload, _ = created['jwt'].split('.')
    claims = json.loads(base64.urlsafe_b64decode(payload + '=' * (-len(payload) % 4)))
    url = base.replace('http://', 'ws://', 1) + '/api/ws'
    for _ in range(8):
        claims.update(iat=int(time.time()) - 1, exp=int(time.time()) + 2)
        body = base64.urlsafe_b64encode(json.dumps(claims).encode()).rstrip(b'=').decode()
        unsigned = header + '.' + body
        signature = base64.urlsafe_b64encode(hmac.new(key.encode(), unsigned.encode(), hashlib.sha256).digest()).rstrip(b'=').decode()
        token = unsigned + '.' + signature
        credentials.append(token)
        with connect(url, origin=origin) as client:
            client.send(envelope(10, field(1, token.encode())))
            assert 16 in decode(client.recv(timeout=5))
            try:
                client.recv(timeout=5)
                raise AssertionError('expired token remained connected')
            except ConnectionClosed as error:
                assert error.rcvd.code == 1001, 'expiry incorrectly disabled reconnect'
    print('WebSocket expiry passed: 8 short-lived sessions close with reconnectable 1001.', flush=True)
