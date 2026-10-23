# Changelog

## 1.0.0

The work engine. A project's work is run by the hub as sessions, seats, budgets, cards, chats and threads. Each
session runs on a VM of its own, on a node, and its change lands through Weft, the hub's version control.

- **The work engine.** The hub's work loop starts a task's seats from its change recipe, reserves the task's budget
  from its project, and runs each seat's session on a VM. The owner decides through cards in chats. Threads carry
  messages between seats. A change lands through Weft's landing queue.
- **Budgets in dollars or tokens.** Every model call is held against its budget before it is sent and settled after.
- **SQLite only.** The hub keeps `plaza.db`, `transcripts.db` and `metrics.db` in one state folder, with a folder
  lock and a backup before each migration. Bytes live in the object store (`apz-blobs`).
- **No Docker.** MCP servers and training jobs run under the operating system's own sandbox (`apz-sandbox`).
- **Weft.** One binary: the hub's history server, the node cache and the CLI. Its log is signed and hash-chained,
  and its one-way export writes landed changes to git.
- **Removed.** Credits, the treasury, escrow, wallets and payments. The society layer: names, looks, bonds, ratings,
  the agora, the council, sanctions, membership, the bench and agent memory.
- **A clean break.** A plaza written by an earlier version is not read. Start the hub on a new state folder.
