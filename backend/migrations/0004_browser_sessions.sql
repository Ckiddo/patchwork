ALTER TABLE patchwork.users ADD COLUMN legacy_exchanged boolean NOT NULL DEFAULT false;
ALTER TABLE patchwork.users ADD COLUMN connection_generation bigint NOT NULL DEFAULT 0 CHECK (connection_generation >= 0);
ALTER TABLE patchwork.sessions ADD COLUMN previous_refresh_hash bytea CHECK (octet_length(previous_refresh_hash)=32);
ALTER TABLE patchwork.sessions ADD COLUMN rotation_id uuid;
ALTER TABLE patchwork.sessions ADD CONSTRAINT rotation_pair CHECK ((previous_refresh_hash IS NULL) = (rotation_id IS NULL));
