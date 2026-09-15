-- Stable keyset pagination; no offset drift while rooms enter or leave the lobby.
CREATE INDEX rooms_waiting_id ON patchwork.rooms(room_id) WHERE phase = 'waiting';
