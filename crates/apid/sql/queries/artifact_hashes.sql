-- Every hash the index names: each version's spec, each compiled script and each data blob, each once. No parameters.
SELECT "spec_hash" FROM "artifact_version"
UNION
SELECT "hash" FROM "artifact_version_scripts"
UNION
SELECT "hash" FROM "artifact_blob"
