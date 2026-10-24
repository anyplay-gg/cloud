# One program, two shapes

The cloud runs as one process for people who host their own and as many nodes for the cloud AnyPlay runs, from the same code. Only two adapters differ between the shapes: the store (SQLite or Postgres) and the news between nodes (inside the process or over NATS). Every route, every rule and every behaviour test is shared, so a self-hosted cloud is never a lesser copy. Large player networks ended up with the same kind of program: Nakama is one server with subsystems, and Open Match 2 folded its services into one binary that scales as pools.

## Considered options

- A separate cluster product: two code bases drift apart, and self-hosters would get the one that is tested less.
- A service per feature from the start: a team of a few people cannot run, version and debug twenty services, and none of the features needs to scale on its own yet. A role flag can split the program later without a rewrite.
- Cluster only: every self-hoster would have to run Postgres and NATS for a cloud that fits in one process.
