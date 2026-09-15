CREATE TABLE patchwork.games (
    game_id uuid PRIMARY KEY,
    room_id uuid NOT NULL REFERENCES patchwork.rooms,
    player0 uuid NOT NULL REFERENCES patchwork.users,
    player1 uuid NOT NULL REFERENCES patchwork.users,
    phase text NOT NULL CHECK (phase IN ('playing','paused','finished','abandoned')),
    rules_version text NOT NULL,
    state_version bigint NOT NULL DEFAULT 0 CHECK (state_version >= 0),
    event_seq bigint NOT NULL DEFAULT 0 CHECK (event_seq >= 0),
    snapshot jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (player0 <> player1)
);
CREATE UNIQUE INDEX games_one_active ON patchwork.games(room_id) WHERE phase IN ('playing','paused');
CREATE INDEX games_recovery ON patchwork.games(phase,updated_at,game_id);
CREATE TABLE patchwork.game_events (
    game_id uuid NOT NULL REFERENCES patchwork.games,
    seq bigint NOT NULL CHECK (seq > 0),
    state_version bigint NOT NULL CHECK (state_version > 0),
    user_id uuid NOT NULL REFERENCES patchwork.users,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (game_id,seq)
);
CREATE TABLE patchwork.command_receipts (
    game_id uuid NOT NULL REFERENCES patchwork.games,
    user_id uuid NOT NULL REFERENCES patchwork.users,
    request_id text NOT NULL CHECK (request_id ~ '^[A-Za-z0-9_-]{1,64}$'),
    payload_hash bytea NOT NULL CHECK (octet_length(payload_hash) = 32),
    state_version bigint NOT NULL CHECK (state_version >= 0),
    response jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (game_id,user_id,request_id)
);
CREATE TABLE patchwork.game_results (
    game_id uuid PRIMARY KEY REFERENCES patchwork.games,
    score0 integer NOT NULL,
    score1 integer NOT NULL,
    winner_seat smallint CHECK (winner_seat IN (0,1)),
    reason text NOT NULL CHECK (reason IN ('completed','resigned','timeout','abandoned')),
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (reason <> 'abandoned' OR winner_seat IS NULL)
);
