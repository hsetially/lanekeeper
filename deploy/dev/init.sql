-- Local development and test bootstrap. Runs once, as the superuser, on an empty data directory.
-- Production gets the same roles from CloudNativePG managed roles (prompt 09); 0001_init.sql never creates roles (Q6).
-- Every password here is for local development only.

CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;

-- lanekeeper_migrator owns the schema and runs migrations. lanekeeper_app is what the hub connects as: it gets DML only,
-- granted table by table in the migrations, and it can never change the schema (S9, S21).
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'lanekeeper_migrator') THEN
    CREATE ROLE lanekeeper_migrator LOGIN PASSWORD 'lanekeeper-migrator-dev-only' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'lanekeeper_app') THEN
    CREATE ROLE lanekeeper_app LOGIN PASSWORD 'lanekeeper-app-dev-only' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
  END IF;
END
$$;

-- PostgreSQL 16 no longer lets every role create objects in schema public.
GRANT CONNECT ON DATABASE lanekeeper TO lanekeeper_migrator, lanekeeper_app;
GRANT USAGE, CREATE ON SCHEMA public TO lanekeeper_migrator;
GRANT USAGE ON SCHEMA public TO lanekeeper_app;
