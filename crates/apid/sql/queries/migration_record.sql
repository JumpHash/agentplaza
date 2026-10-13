-- Records one applied migration; runs in the same transaction as the migration itself.
-- $1 = version (int), $2 = name (text), $3 = sha256 checksum of the file (hex text)
INSERT INTO schema_migrations (version, name, checksum) VALUES ($1, $2, $3)
