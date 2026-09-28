-- Per-service / per-datastore resource limits (NULL = the server default).
ALTER TABLE services ADD COLUMN memory_limit_mb INTEGER;
ALTER TABLE services ADD COLUMN cpu_limit REAL;
ALTER TABLE datastores ADD COLUMN memory_limit_mb INTEGER;
ALTER TABLE datastores ADD COLUMN cpu_limit REAL;
