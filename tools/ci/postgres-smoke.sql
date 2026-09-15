-- Infrastructure check only. Application migrations and SQLx tests follow in T06+.
DO $$
BEGIN
    IF current_database() <> 'patchwork_ci' OR current_user <> 'patchwork_ci' THEN
        RAISE EXCEPTION 'refusing to run outside the disposable CI database';
    END IF;
END $$;

BEGIN;
CREATE TEMP TABLE foundation_probe (id uuid PRIMARY KEY, state jsonb NOT NULL);
INSERT INTO foundation_probe VALUES ('00000000-0000-0000-0000-000000000001', '{"version": 1}');
DO $$
BEGIN
    IF (SELECT count(*) FROM foundation_probe WHERE state->>'version' = '1') <> 1 THEN
        RAISE EXCEPTION 'PostgreSQL type/transaction probe failed';
    END IF;
END $$;
ROLLBACK;
