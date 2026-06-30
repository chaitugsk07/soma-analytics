-- Idempotent schema bootstrap. Runs on every `up()` call before any versioned
-- migrations. Safe to re-run: all statements are guarded with IF NOT EXISTS /
-- CREATE OR REPLACE.
--
-- DB-206: default privileges give soma_write_user DML access and
-- soma_read_user SELECT access to all NEW tables created in this schema.
-- TODO: match platform role names — soma-vault and soma-audit also use
-- soma_write_user / soma_read_user; confirm against the shared-instance config.

CREATE SCHEMA IF NOT EXISTS "soma_analytics";

CREATE OR REPLACE FUNCTION "soma_analytics".fn_update_timestamp()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$;

-- Default privileges for new tables (db-standards DB-206).
ALTER DEFAULT PRIVILEGES IN SCHEMA "soma_analytics"
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO soma_write_user;

ALTER DEFAULT PRIVILEGES IN SCHEMA "soma_analytics"
    GRANT SELECT ON TABLES TO soma_read_user;
