-- Runs once, when the postgres volume is first created (compose.yaml mounts this directory at
-- /docker-entrypoint-initdb.d). `cargo test` (ORCH_TEST_DATABASE_URL) needs this database.
CREATE DATABASE orch_test;
