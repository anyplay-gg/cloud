# One store, many regions

The cloud is a control plane. Game traffic goes from player to player or through a relay and never through the cloud, so the latency a player feels in a game comes from the path to the other player and to the relay, not from the cloud. What the cloud's latency touches is signing in, a presence change reaching a friend, a join and a message, where a tenth of a second more is not felt.

So the network grows into other regions in two steps, and only the first is decided now:

1. **Regions without cells.** Each region gets its own relays (the data plane, where distance matters), its own cloud nodes that hold the players' event connections, and its own NATS cluster, joined to the others by NATS gateways. All regions share one store: one PostgreSQL primary in the EU with a synchronous standby there, and read replicas in the other regions. A node writes to the primary and reads from its replica where a stale answer cannot hurt (gamer cards, profiles, rarity, another player's trophies); everything a client reads right after its own write, and every check inside a write, goes to the primary. Revisions already make a late copy of a session or a presence harmless.
2. **Cells, later.** When one primary is no longer enough, accounts are split into cells by account ID. Each cell has its own store and nodes, and a thin global directory maps an account to its cell. This step is not designed further until the numbers call for it.

## Why not cells per region now

Riot runs one chat cluster per region, and players in different regions cannot be friends there ([Riot, chat servers](https://www.riotgames.com/en/news/chat-service-architecture-servers)). Friends across regions are part of what AnyPlay is for. Cells with a shared directory solve that, at the price of cross-cell friendships, a router, a rebalancer that moves accounts between cells and deploys that go cell by cell ([AWS, cell-based architecture](https://docs.aws.amazon.com/wellarchitected/latest/reducing-scope-of-impact-with-cell-based-architecture/cell-migration.html)). Slack took a year and a half to move to cells after an outage ([Slack Engineering](https://slack.engineering/slacks-migration-to-a-cellular-architecture/)). That cost buys isolation AnyPlay does not need yet: its data is small and relational, and one primary with replicas carries it to the low millions of players.

## When to move to step 2

Any one of these, held for a week at peak:

- the primary's CPU above 60 % or its write latency (p99) above 50 ms;
- replication lag in any region above 5 seconds;
- the connection count to the primary at the pooler's limit after tuning;
- an outage of the EU region that cost more than the error budget of a quarter.

The load harness (stage 6) measures the first three before any of them happens in production.

## Consequences

- Writes from other regions travel to the EU: about 90 ms more from the US east coast, about 250 ms more from East Asia, per round trip. A join or a message takes a few round trips. Code that writes keeps its statements in one transaction and one round trip where it can.
- An outage of the EU region stops every write everywhere. Nodes in other regions answer `503` with `Retry-After` at once, as they do today when the store is down. Games already running go on: their tunnels need no cloud, and relay tickets are valid for 24 hours.
- Player data, logs and backups stay in the EU; the replicas outside it are copies. Holding copies outside the EU is a transfer that needs a legal basis; it is on the list for legal review.
- Presence and sessions stay in the shared store rather than in each region, so a friend in another region reads them like a friend next door. Their news crosses regions through the NATS gateways.

## Considered options

- **A store per region with replication between them** (multi-primary): conflicts on friends, blocks and session capacity, which the code avoids today by having one writer per row.
- **A distributed SQL database** (CockroachDB, YugabyteDB): writes in several regions without a single primary. CockroachDB needs an enterprise licence for self-hosters since November 2024 ([ADR 0003](0003-postgres-for-a-cluster.md)); YugabyteDB was not evaluated. Both add a system much harder to run than PostgreSQL for a gain the control plane does not need.
- **Regional cells from the start**: the Riot shape; rejected above.
