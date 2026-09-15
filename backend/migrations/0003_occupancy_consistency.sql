-- Deferred checks allow membership, tickets and occupancy to change atomically.
CREATE FUNCTION patchwork.check_occupancy() RETURNS trigger LANGUAGE plpgsql
SET search_path = pg_catalog, patchwork AS $$
DECLARE ids uuid[]; id uuid; s text; ticket uuid; room uuid; members bigint; active bigint;
BEGIN
    IF TG_OP = 'INSERT' THEN ids := ARRAY[NEW.user_id];
    ELSIF TG_OP = 'DELETE' THEN ids := ARRAY[OLD.user_id];
    ELSE ids := ARRAY[OLD.user_id,NEW.user_id]; END IF;
    FOREACH id IN ARRAY ids LOOP
        SELECT state,ticket_id,room_id INTO s,ticket,room FROM patchwork.player_occupancy WHERE user_id=id;
        SELECT count(*) INTO members FROM patchwork.room_members WHERE user_id=id;
        SELECT count(*) INTO active FROM patchwork.matchmaking_tickets WHERE user_id=id AND status IN ('queued','reserved');
        IF (s IS NULL OR s IN ('idle','operation')) AND (members <> 0 OR active <> 0)
           OR s='room' AND (members <> 1 OR active <> 0)
           OR s='queue' AND (members <> 0 OR active <> 1 OR NOT EXISTS (
               SELECT 1 FROM patchwork.matchmaking_tickets WHERE ticket_id=ticket AND user_id=id AND status IN ('queued','reserved')))
        THEN RAISE EXCEPTION USING ERRCODE='23514', MESSAGE='player occupancy invariant violated'; END IF;
    END LOOP;
    RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER occupancy_consistent AFTER INSERT OR UPDATE OR DELETE ON patchwork.player_occupancy
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION patchwork.check_occupancy();
CREATE CONSTRAINT TRIGGER members_consistent AFTER INSERT OR UPDATE OR DELETE ON patchwork.room_members
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION patchwork.check_occupancy();
CREATE CONSTRAINT TRIGGER tickets_consistent AFTER INSERT OR UPDATE OR DELETE ON patchwork.matchmaking_tickets
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION patchwork.check_occupancy();

-- Default privileges grant application DML to business tables, not migration metadata.
REVOKE INSERT,UPDATE,DELETE ON patchwork._sqlx_migrations FROM patchwork_app;
