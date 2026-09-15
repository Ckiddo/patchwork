CREATE SCHEMA IF NOT EXISTS patchwork;
REVOKE ALL ON SCHEMA patchwork FROM PUBLIC;

CREATE TABLE patchwork.users (
    user_id uuid PRIMARY KEY,
    nickname text NOT NULL CHECK (char_length(btrim(nickname)) BETWEEN 1 AND 20),
    created_at timestamptz NOT NULL DEFAULT now(),
    auth_version bigint NOT NULL DEFAULT 0 CHECK (auth_version >= 0)
);
CREATE TABLE patchwork.sessions (
    session_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES patchwork.users,
    refresh_hash bytea NOT NULL UNIQUE CHECK (octet_length(refresh_hash) = 32),
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    CHECK (expires_at > created_at)
);
CREATE INDEX sessions_user ON patchwork.sessions(user_id);
CREATE INDEX sessions_expiry ON patchwork.sessions(expires_at) WHERE revoked_at IS NULL;

CREATE TABLE patchwork.rooms (
    room_id uuid PRIMARY KEY,
    code text NOT NULL UNIQUE CHECK (code ~ '^[A-Z0-9]{6,10}$'),
    mode text NOT NULL CHECK (char_length(mode) BETWEEN 1 AND 32),
    rules_version text NOT NULL CHECK (char_length(rules_version) BETWEEN 1 AND 64),
    password_hash text,
    phase text NOT NULL DEFAULT 'waiting' CHECK (phase IN ('waiting','starting','playing','finished','closed')),
    owner_id uuid REFERENCES patchwork.users,
    version bigint NOT NULL DEFAULT 0 CHECK (version >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (phase = 'closed' OR owner_id IS NOT NULL)
);
CREATE INDEX rooms_joinable ON patchwork.rooms(created_at,room_id) WHERE phase = 'waiting';
CREATE TABLE patchwork.room_members (
    room_id uuid NOT NULL REFERENCES patchwork.rooms,
    seat smallint NOT NULL CHECK (seat IN (0,1)),
    user_id uuid NOT NULL REFERENCES patchwork.users,
    join_seq bigint GENERATED ALWAYS AS IDENTITY,
    ready boolean NOT NULL DEFAULT false,
    connection_generation bigint NOT NULL DEFAULT 0 CHECK (connection_generation >= 0),
    disconnected_at timestamptz,
    PRIMARY KEY (room_id,seat),
    UNIQUE (room_id,user_id),
    UNIQUE (user_id)
);
ALTER TABLE patchwork.rooms ADD CONSTRAINT owner_is_member
    FOREIGN KEY (room_id,owner_id) REFERENCES patchwork.room_members(room_id,user_id)
    DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE patchwork.matchmaking_tickets (
    ticket_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES patchwork.users,
    mode text NOT NULL CHECK (char_length(mode) BETWEEN 1 AND 32),
    rules_version text NOT NULL CHECK (char_length(rules_version) BETWEEN 1 AND 64),
    join_seq bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    joined_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL CHECK (expires_at > joined_at),
    connection_generation bigint NOT NULL CHECK (connection_generation >= 0),
    status text NOT NULL CHECK (status IN ('queued','reserved','matched','cancelled','expired')),
    room_id uuid REFERENCES patchwork.rooms,
    UNIQUE (ticket_id,user_id),
    CHECK ((status = 'matched') = (room_id IS NOT NULL))
);
CREATE UNIQUE INDEX tickets_one_active ON patchwork.matchmaking_tickets(user_id)
    WHERE status IN ('queued','reserved');
CREATE INDEX tickets_fifo ON patchwork.matchmaking_tickets(mode,rules_version,join_seq)
    WHERE status = 'queued';
CREATE INDEX tickets_expiry ON patchwork.matchmaking_tickets(expires_at)
    WHERE status IN ('queued','reserved');

CREATE TABLE patchwork.operation_receipts (
    operation_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES patchwork.users,
    request_id text NOT NULL CHECK (request_id ~ '^[A-Za-z0-9_-]{1,64}$'),
    payload_hash bytea NOT NULL CHECK (octet_length(payload_hash) = 32),
    response jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id,request_id)
);
CREATE TABLE patchwork.player_occupancy (
    user_id uuid PRIMARY KEY REFERENCES patchwork.users,
    state text NOT NULL DEFAULT 'idle' CHECK (state IN ('idle','queue','room','operation')),
    ticket_id uuid,
    room_id uuid,
    operation_id uuid REFERENCES patchwork.operation_receipts DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (ticket_id,user_id) REFERENCES patchwork.matchmaking_tickets(ticket_id,user_id),
    FOREIGN KEY (room_id,user_id) REFERENCES patchwork.room_members(room_id,user_id) DEFERRABLE INITIALLY DEFERRED,
    CHECK (
        (state = 'idle' AND num_nonnulls(ticket_id,room_id,operation_id) = 0) OR
        (state = 'queue' AND ticket_id IS NOT NULL AND num_nonnulls(room_id,operation_id) = 0) OR
        (state = 'room' AND room_id IS NOT NULL AND num_nonnulls(ticket_id,operation_id) = 0) OR
        (state = 'operation' AND operation_id IS NOT NULL AND num_nonnulls(ticket_id,room_id) = 0)
    )
);
CREATE INDEX occupancy_room ON patchwork.player_occupancy(room_id) WHERE room_id IS NOT NULL;
