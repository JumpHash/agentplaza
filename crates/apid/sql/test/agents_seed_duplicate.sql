-- Test only: an agents blob that repeats cid 1000, which the load must refuse.
INSERT INTO plaza_state (component, data, hash, updated_at) VALUES ('agents', '[
  {"cid": 1000, "name": "ada", "role": "worker", "created_at_ms": 1700000000000},
  {"cid": 1000, "name": "ada again", "role": "worker", "created_at_ms": 1700000000000}
]', 0, now());
