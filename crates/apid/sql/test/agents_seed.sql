-- Test only: a plaza_state agents blob of three agents (one retired, one without a model).
INSERT INTO plaza_state (component, data, hash, updated_at) VALUES ('agents', '[
  {"cid": 1000, "name": "ada", "role": "worker", "model": "openrouter/x", "species": "fox", "species_at_ms": 1700000000000, "state": "active", "created_at_ms": 1700000000000, "updated_at_ms": 1700000100000},
  {"cid": 1001, "name": "bo", "role": "orchestrator", "species": "owl", "state": "retired", "created_at_ms": 1700000000000, "retired_at_ms": 1700000200000, "updated_at_ms": 1700000200000},
  {"cid": 1002, "name": "cy", "role": "researcher", "species": "kite", "created_at_ms": 1700000050000}
]', 0, now());
