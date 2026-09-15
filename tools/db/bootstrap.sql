-- Run once against an EMPTY dedicated database using an administrative account.
-- psql -X -v ON_ERROR_STOP=1 -v db_name=patchwork_dev -f tools/db/bootstrap.sql
-- Password values come from process-local environment; do not pass them as CLI args.
\getenv migrate_password PATCHWORK_MIGRATE_PASSWORD
\getenv app_password PATCHWORK_APP_PASSWORD
CREATE ROLE patchwork_owner NOLOGIN;
CREATE ROLE patchwork_migrate LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE PASSWORD :'migrate_password';
GRANT patchwork_owner TO patchwork_migrate;
CREATE ROLE patchwork_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE PASSWORD :'app_password';
CREATE DATABASE :"db_name" OWNER patchwork_owner;
REVOKE ALL ON DATABASE :"db_name" FROM PUBLIC;
GRANT CONNECT ON DATABASE :"db_name" TO patchwork_app,patchwork_migrate;
\connect :db_name
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
CREATE SCHEMA patchwork AUTHORIZATION patchwork_owner;
GRANT USAGE ON SCHEMA patchwork TO patchwork_app;
ALTER DEFAULT PRIVILEGES FOR ROLE patchwork_owner IN SCHEMA patchwork
    GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO patchwork_app;
ALTER DEFAULT PRIVILEGES FOR ROLE patchwork_owner IN SCHEMA patchwork
    GRANT USAGE,SELECT ON SEQUENCES TO patchwork_app;
