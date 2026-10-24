# Shared state lives in the store

Presence, sessions, invitations, sign-in challenges and virtual address leases move from the process's memory into the store, in a single process too. A node keeps only its own sockets and caches that it can refill. So both shapes run one code path, any node can answer any request, and a node can die without taking state with it. The price is a database write for each connect, presence change and session change. SQLite and Postgres both carry that at the rates a player network produces; the load harness of stage 6 measures how far.

Concurrent changes to one session are ordered by touching its row first, which locks it in both databases until the transaction ends. Sweeps run on every node as conditional statements, so no node has to be elected to run them.

## Considered options

- State in each node's memory, with players routed to the node that owns their sessions: the fastest path, but it needs ownership, hand-over and routing between nodes, and it fails exactly when a node dies.
- State in the NATS key-value store: fast and shared, but a second source of truth with weaker guarantees than the database ([ADR 0004](0004-nats-carries-news-not-state.md)).
- Memory in a single process and the store only in a cluster: two implementations of the hardest logic in the cloud, each tested half as much.
