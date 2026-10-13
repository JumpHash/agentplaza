-- Every applied migration, in version order: version, name, sha256 checksum (hex). No parameters.
SELECT version, name, checksum FROM schema_migrations ORDER BY version
