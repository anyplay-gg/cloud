# SQLite for the durable data

The cloud keeps its durable data in one SQLite file in WAL mode: accounts, devices, friends, requests, blocks, reports and bans. That data is small and written rarely: a row when someone signs up, befriends, blocks or reports. Presence, sessions, invitations, sign-in challenges and virtual address leases change all the time and are worth nothing after a restart, so they live in memory and never touch the disk. One file means that `docker compose up` needs no database server, a backup is a copy of the data folder, and anyone can host a cloud without learning to run a database. The cloud is one process; if a single process ever stops being enough, the storage code in `db.rs` is the only part that has to change.

## Considered options

- Postgres: handles many writers and several cloud processes, but every operator would run, secure and back up a second service for data that fits in one file. We can move to it when a cloud outgrows one process.

## Later

[ADR 0003](0003-postgres-for-a-cluster.md) keeps SQLite for a single process and adds Postgres for a cluster. [ADR 0005](0005-shared-state-lives-in-the-store.md) moves presence, sessions, invitations, challenges and leases from memory into the store, so the line above between durable data and memory no longer holds.
