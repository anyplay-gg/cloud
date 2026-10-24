# Architecture

How the cloud grows from one process on a home server to a cluster that carries a whole player network, and why it is built that way. The decisions are in [docs/adr](adr); the words are those of the network's [CONTEXT.md](https://github.com/anyplay-gg/network/blob/main/CONTEXT.md).

The patterns here come from what large player networks have said in public: talks, engineering blogs, postmortems and documentation. None of it comes from anyone's internal or leaked material, and the cloud never stands in for anyone's servers ([the clean line](https://github.com/anyplay-gg/network/blob/main/docs/adr/0003-the-clean-line.md)).

## One program, two shapes

```text
 single node                               cluster

 ┌──────────────────────────┐              players: HTTPS API + one WebSocket each
 │ anyplay-cloud            │                              │
 │  every role              │              ┌───────────────▼───────────────┐
 │  SQLite file             │              │   load balancer, TLS (L7)     │
 │  news in process         │              └──┬───────────┬────────────┬───┘
 └──────────────────────────┘                 │           │            │
   docker compose up                      ┌───▼───┐   ┌───▼───┐    ┌───▼───┐
   for self-hosters, development          │node 1 │   │node 2 │ …  │node N │  anyplay-cloud
   and every test                         └─┬───┬─┘   └─┬───┬─┘    └─┬───┬─┘  holds only sockets
                                            │   └──────┼───┴─news───┘   │    and caches
                                            │          │  NATS core     │
                                          ┌─▼──────────▼────────────────▼─┐
                                          │ PostgreSQL: primary, replicas │  every shared fact
                                          │ (CloudNativePG)               │
                                          └───────────────────────────────┘

 relays: anyplay-relay (iroh-relay + ticket check), two or more per region, never call the cloud
```

Both shapes run the same code. Two adapters differ, and nothing else ([ADR 0002](adr/0002-one-program-two-shapes.md)):

| Adapter | Single node | Cluster |
|---|---|---|
| **Store**: durable and shared state | SQLite in WAL mode | PostgreSQL ([ADR 0003](adr/0003-postgres-for-a-cluster.md)) |
| **News**: who must be told about a change | channels inside the process | NATS core subjects ([ADR 0004](adr/0004-nats-carries-news-not-state.md)) |

The behaviour tests run against both stores, so a self-hosted cloud and AnyPlay's cloud behave the same way.

## Where state lives

A node holds nothing that another node needs. Any node can answer any request, and a node can die at any time ([ADR 0005](adr/0005-shared-state-lives-in-the-store.md)).

| State | Lives in | Why |
|---|---|---|
| Accounts, devices, refresh tokens, friends, requests, blocks, reports, bans | store | durable |
| Sign-in challenges and sign-up permissions | store | a code requested through one node is entered through another |
| Presence: which devices hold an event connection, through which node, and the activity | store | friends on other nodes read it; a dead node's devices count as gone |
| Sessions, members, invitations | store | joins from many nodes must see one capacity and one member list |
| Parties, party members, party invitations | store | one party per account and 8 members, whichever node writes |
| Conversations, messages, read marks | store | durable for 30 days; the conversation's counter gives each message its number |
| Virtual address leases | store | unique across the network |
| Event connections (the sockets) | the node that holds them | they cannot move |
| Who is signed in, who is banned | a cache in each node, filled from the store | checked on every request; changes arrive as news |
| Rate-limit counters | each node | cheap and approximate; a shared limiter can come later |

## How news travels

Every change is a transaction in the store. After it commits, the node that made it publishes a short note to each account that has to hear about it, on the subject `anyplay.account.<id>`. A node subscribes to that subject while it holds a connection of the account, and passes the note to the connection. In a single node the note goes straight to the connection.

Notes are delivered at most once, and that is enough, because nothing depends on a single note:

- Events that carry an object (a session, a presence) carry its **revision**. A client keeps the highest revision it has seen and drops an older copy, however late it arrives.
- Events that carry no object (`friends`) make the client read the current state.
- A new event connection starts with `welcome`, and the client reads its lists after it.

The pattern is the one Riot's messaging service and Roblox's notification stream describe: small notes, versions, and the client reads the truth. Fan-out is on write and goes only to accounts that are online, which the store knows; with at most 500 friends, one presence change sends at most 500 notes.

## Concurrency across nodes

- **One session, one writer at a time.** Every change to a session starts by touching its row (`UPDATE sessions SET revision = revision + 1 … RETURNING …`). That holds a row lock in Postgres and the write lock in SQLite until the transaction ends, so blocks, visibility, the version guard and capacity are checked against a state nobody else is changing. The same statement yields the new revision.
- **One conversation, one counter.** A message takes the next number with an `UPDATE` of its conversation's row inside the transaction that stores it. The row lock serializes writers on all nodes, so numbers are never used twice, and a lower number is always committed before a higher one. A client that holds number N asks for what comes after N and misses nothing. Events for messages are pointers (conversation and number); a party is a versioned state like a session ([network ADR 0013](https://github.com/anyplay-gg/network/blob/main/docs/adr/0013-messages-are-numbered-per-conversation.md)).
- **One party, one writer at a time.** Every change to a party starts by touching its row, as for a session. The unique account in `party_members` keeps an account in one party whichever node writes.
- **Sweeps without a leader.** Presence grace, expired invitations (session and party), party members gone too long, messages past their retention, challenges and leases, and the connections of dead nodes are swept by every node. Each step is one conditional statement, so it takes effect once, and the node whose statement changed rows sends the news.
- **Dead nodes.** Each node writes a heartbeat. The connections of a node whose heartbeat is older than its timeout count as closed, and the usual grace period follows.

## Deploys and failures

The worst outages in public postmortems came from load spikes and reconnect storms, not from data size: Fortnite's at 3.4 million players in 2018 and the 73 hours Roblox was down in 2021. Discord kept a stampede of five million sessions in check by failing fast instead of queueing. So:

- A node told to stop (`SIGTERM`) leaves the load balancer first (`/readyz` fails), then closes its event connections over a drain window with close code 1012. Clients reconnect at a random moment inside that window, not all at once.
- `/healthz` answers while the process runs; `/readyz` while the store is reachable and the node is not draining; `/metrics` exposes Prometheus metrics.
- When the store is unreachable, requests fail fast with `503` and `Retry-After` instead of queueing.
- Losing NATS loses notes, not state; clients that reconnect read the truth.

## Regions

The cloud is a control plane: game traffic never passes through it, so a region far from the cloud slows down a sign-in or a join by a fraction of a second, not a game. The first cluster runs in one region in the EU. Other regions come in two steps ([ADR 0006](adr/0006-one-store-many-regions.md)):

1. **Regions without cells.** Each region runs its own relays, its own nodes for the players' connections and its own NATS cluster, joined by NATS gateways. All regions share one store: the primary in the EU, read replicas elsewhere for reads that may be a little late. Friendships, presence and sessions work across regions as they do inside one.
2. **Cells**, when one primary is no longer enough: accounts split by ID, each cell with its own store, and a thin directory in front. The ADR names the numbers that start this step.

When the EU region is down, writes fail fast everywhere, and games already running go on, because their tunnels and relay tickets need no cloud.

## Relays

Game traffic goes directly between players where the network allows it and through a relay otherwise ([network ADR 0002](https://github.com/anyplay-gg/network/blob/main/docs/adr/0002-tunnels-on-iroh.md)). A relay admits only an endpoint that holds a ticket the cloud signed for it ([network ADR 0007](https://github.com/anyplay-gg/network/blob/main/docs/adr/0007-relays-check-signed-tickets.md)), and it cannot read the traffic it carries.

- Relays cost bandwidth, not CPU. They run on servers with cheap traffic, at least two per region, outside the cluster.
- What a relay enforces, all on by default and set in [the relay's settings](configuration.md#the-relay): each client may send 1 MiB per second (a burst of 2 MiB); an account may hold 16 connections at once; a relay carries 10,000 clients; and a ticket that is valid for more than 25 hours is refused, because the cloud issues 24. The account comes from the signed ticket, so the quotas follow the person and not the endpoint ID, which anyone can make new. A ticket is checked once, when the client connects: revoking an account works through the short ticket life and the cloud no longer issuing new tickets, and a relay can be made to forget a client by restarting it.
- The cloud limits how often an account may ask for tickets: 20 per hour. A refusal is the `rate_limited` error with `Retry-After`. Like the other limits of the cloud, the count is kept in memory per node.
- Refusals are counted, never logged per packet or per connection. The relay's metrics port serves clients, bytes in and out, ticket checks by result and refusals by reason, with no account or endpoint IDs in any label.
- A player who hides their IP address goes through relays with every peer. The cloud never publishes a player's direct addresses.
- The share of tunnels that go direct decides how many relays a region needs. A relay cannot see it: it sees clients and bytes, not which tunnels avoided it. The net service knows the path of each tunnel and writes a line when it changes (`anyplay::path` on standard error, with running totals). Reading that line for real players needs a report to the cloud that the specification does not have yet; until then the relay's bytes in and out are the cost measure.

## Security

- All nodes sign with one key, read from a secret file. A key ID in each token will allow rotation without signing everybody out.
- Containers run as non-root with a read-only root file system, under Kubernetes' restricted pod security profile, with network policies that deny by default.
- EU data protection and platform law (GDPR, Digital Services Act): draft documents for legal review, built from what the code stores, are in [docs/compliance](compliance/README.md).
- Planned: passkeys as the main way to sign in, with e-mail codes for verification and recovery, and refresh tokens bound to a key on the device (DPoP, RFC 9449).

## Testing

- Behaviour tests through the HTTP API and the event stream, on SQLite and on Postgres.
- A cluster test: two nodes, one NATS and one Postgres; players on different nodes see each other's presence, sessions and invitations.
- The network's end-to-end test: cloud, relay, two net services and two fake games in one process.
- A load harness that plays many players at once, with login waves, presence churn and reconnect storms, not only steady load.
- Nightly fuzzing of the token check ([fuzz/](../fuzz/README.md)).

## Stages

| Stage | What | State |
|---|---|---|
| 1 | Everything shared moves into the store; the store runs on SQLite and Postgres; tests on both | done |
| 2 | News over NATS; the cluster test | done |
| 3 | Revisions on sessions and presence; clients spread their reconnects | done |
| 4 | Operations: health, readiness, metrics, drain, JSON logs | done |
| 5 | Kubernetes: a Helm chart, tried on a kind cluster in CI ([guide](kubernetes.md)) | done |
| 6 | Load harness and first numbers: [docs/LOAD.md](LOAD.md) (one laptop, up to 5,000 players; a real cluster is not measured yet) | first numbers |
| 7 | Regions and cells | |

### Known limits

- Virtual addresses come from `198.18.0.0/15`, one per online account, which caps a cloud at about 131,000 players online at once. Addresses only have to differ inside a session, so the cloud API will move to per-session addresses before that limit is near.
- Rate limits count per node, so a cluster of N nodes allows up to N times the limit to a client that spreads its requests. A load balancer that picks the node by the client's address keeps that close to the limit; a shared limiter removes it.
- A node learns of a device signed in elsewhere from the store, at its first request. When a device signs out, the other nodes hear of it over NATS; a node that misses that note accepts the device's access token until it expires, at most 15 minutes. Bans are read again from the store at every sweep.
