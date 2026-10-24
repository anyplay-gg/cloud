# Load

`anyplay-load` (crate `crates/load`) plays many players against the cloud through its real HTTP API and event WebSockets, and measures what the cloud and the players see. This is stage 6 of the [architecture](ARCHITECTURE.md#stages): a harness and the first numbers. The numbers are from one laptop. They show where this cloud bends first, not what a real cluster carries.

Public postmortems (Fortnite 2018 at 3.4 million players, Roblox 2021) put the outages in login waves, churn and reconnect storms rather than steady load, so the scenarios model those.

## What it plays

A run always starts with the first two, then runs the scenarios named in `--scenarios` (default `presence,sessions,storm`; the storm goes last because it can take a node down).

| Scenario | What the players do | What it measures |
|---|---|---|
| login wave | `--players` players sign up (or, for `--returning` of them, sign in) at `--rate` per second: `auth/email`, `auth/code`, `auth/signup`, the event WebSocket, then `GET /v1/friends` as a client does after the welcome. | Latency per call, time from the first request to the welcome (`e2e_login`). |
| friend graph | Each player befriends the next `--friends`/2 players on a ring, so everyone ends with about `--friends` friends: a request by handle, then the accept. | Throughput and latency of the friend calls. |
| presence churn (`presence`) | `--presence-rate` activity changes per second, over random players, for `--presence-seconds`. Each change carries its send time in the activity's `version`. | Time from the change until each online friend's connection receives the `presence` event: p50, p95, p99. Events expected against events received. |
| sessions (`sessions`) | `--session-rate` sessions per second: a leader creates one of `--session-size` members, friends join, it is held `--session-hold` seconds, then all leave. | Latency of create, join and leave. |
| gamer cards (`cards`) | `--cards-rate` calls per second for `--cards-seconds`. Each call is by a random player and asks for the cards of `--cards-size` accounts (at most 100): the player's friends first, other players to fill up. | Time per call: p50, p95, p99. Every call must answer 200 with one card per account asked; any other count is shown as `wrong_count`. |
| reconnect storm (`storm`) | `--storm drop`: every connection breaks at once, with no close frame. `stop-node`: node 1 drains and closes with 1001 (cluster only). `kill-node`: node 1 vanishes (cluster only). Players reconnect after the rule in `--reconnect`. | Seconds from the start until 50, 90, 95, 99 and 100 percent of the broken players are back, and the errors on the way. |

Rate-driven scenarios are open loop: operations start on schedule whatever the cloud does. When `--max-in-flight` are already running, further ones are counted as `skipped_saturated` rather than queued, so an overloaded cloud shows up in the numbers instead of slowing the clock.

Reconnects follow `spec/cloud-api.md`: after 1001 or 1012 a random moment within 10 s; after any other break, half the pause plus a random part of up to the whole pause, from 1 s doubling to 30 s (`--reconnect spec`). `--reconnect immediate` reconnects at once, the worst case of a client that ignores the specification. After the welcome a player fetches `GET /v1/friends`, as the specification tells clients.

## Running it

Build in release mode; a debug build measures the debug build.

```sh
cargo build --release -p anyplay-load
```

**In process, one node on SQLite** (the cloud runs inside the harness, with a sign-in code sink and rate limits raised, because every player comes from 127.0.0.1):

```sh
target/release/anyplay-load --players 5000 --rate 200 --presence-rate 200 \
  --session-rate 8 --json out.json
```

**In process, a cluster** (`--database-url` names a PostgreSQL server; the harness creates a database of its own on it and drops it at the end. `--nats-url` is NATS. Both can come from `ANYPLAY_LOAD_DATABASE_URL` and `ANYPLAY_LOAD_NATS_URL`):

```sh
target/release/anyplay-load --nodes 2 --players 5000 --rate 200 \
  --database-url postgres://postgres:test@127.0.0.1:54329/postgres \
  --nats-url nats://127.0.0.1:4222 --presence-rate 200 --session-rate 30
```

**One scenario**: `--scenarios storm`, `--scenarios presence`, and so on. The login wave and the friend graph still run first, because every scenario needs players with friends. Storm variants: `--storm drop|stop-node|kill-node`, `--reconnect spec|immediate`, `--storm-target 0.95`, `--storm-timeout 120`.

**An external cloud** (`--url`, repeatable for several nodes): the harness cannot read a mail sink there, so run the cloud with `mail = "log"` writing to a file and pass `--mail-log FILE`. Give `--metrics-url` for each node's `/metrics` and `--pid` for each node's process to get its memory and CPU. The cloud applies its real rate limits, which allow 10 sign-in mails per IP address per 15 minutes, so from one machine this works for a handful of players only (tried with 8). A larger external run needs a cloud that raises or lifts those limits, or many source IPs.

```sh
target/release/anyplay-load --url http://127.0.0.1:18080 --metrics-url http://127.0.0.1:18081 \
  --mail-log cloud.log --pid 1234 --players 8 --friends 2
```

`--help` lists every setting. A run prints one table per scenario and writes the same data as JSON with `--json`: `machine`, `settings` (without the database and NATS URLs), `commit`, `date`, and per scenario `ops` (count, ok, p50/p95/p99/max in ms and status counts per operation), `http_status`, `events`, `delivery`, `storm`, `server` (what the nodes' `/metrics` say changed), `process` and `pool`.

Reading the table: `e2e_login` is the time from the first request of a login to the welcome. `ws_connect` includes the wait for the welcome. Status `no answer` is a request that got no HTTP answer. `server side` lists the routes with the most handler time, from the nodes' `/metrics`: a long mean there that the single request does not need means the handler waited (for a lock or a store connection). `store connections busy` is sampled four times a second from the in-process pools.

## The numbers

Machine: AMD Ryzen AI 9 365 laptop, 20 logical cores, 31.3 GB, Windows 11, on mains power. Date 2026-10-24. Cloud commit `a4a5c67` (main) with the harness of this change; release build. Harness: 6 worker threads. Each in-process node: 4 worker threads and 16 store connections. PostgreSQL 54329 and NATS 4222 are local servers on the same laptop (so they compete for the same cores). All players come from 127.0.0.1. The raw JSON is in [`crates/load/results/2026-10-24/`](../crates/load/results/2026-10-24).

Common settings: 5,000 players, 200 logins per second, 10 friends each, 200 activity changes per second for 30 s. The memory and CPU figures are of the whole process, harness and in-process cloud together, because they share one; they are an upper bound for the cloud.

### A. One node, SQLite

```sh
anyplay-load --players 5000 --rate 200 --presence-rate 200 --session-rate 8 --json A-sqlite-5000.json
```

| Scenario | Seconds | HTTP calls/s | Result |
|---|---|---|---|
| login wave | 25.3 | 791 | 5,000 signed in, 5,000 connections held. `e2e_login` p50 12.7 ms, p95 364 ms, p99 1,311 ms. `auth_code` p99 482 ms. All 200. |
| friend graph | 52.7 | 949 | 25,000 friendships. `friend_request` p50 267 ms, p99 344 ms. |
| presence churn | 30.2 | 198 | 5,998 changes, 59,980 of 59,980 events delivered. Delivery p50 2.6 ms, **p95 282 ms, p99 870 ms**, max 2.6 s. |
| sessions (8/s) | 31.0 | 62 | 239 created, 717 joins. create p99 4 ms, join p99 59 ms, leave p99 50 ms. All succeeded. |
| storm, `drop`, spec | 9.3 | 539 | 5,000 of 5,000 back. 50% after 7.2 s, 95% after 8.5 s, 100% after 8.75 s. 8 requests failed (see below). |

Process: peak 1.6 GB, CPU 1.7 cores in the wave, 3.4 cores (peak 4.8) in the storm.

### B. Two nodes, PostgreSQL and NATS

```sh
anyplay-load --nodes 2 --players 5000 --rate 200 --presence-rate 200 --session-rate 30 \
  --database-url ... --nats-url ... --json C-pg2-5000.json
```

| Scenario | Seconds | HTTP calls/s | Result |
|---|---|---|---|
| login wave | 25.0 | 799 | 5,000 in. `e2e_login` p50 16 ms, **p95 38 ms, p99 75 ms**. |
| friend graph | 83.0 | 603 | 25,000 friendships. `friend_request` p50 408 ms, p99 1,096 ms. |
| presence churn | 33.0 | 182 | 5,998 changes, 59,970 of 59,980 events delivered (99.98%). Delivery p50 3.5 ms, **p95 9.7 ms, p99 181 ms**, max 378 ms. |
| sessions (30/s) | 31.0 | 232 | 899 created, 2,697 joins, all succeeded. create p99 22 ms, join p99 25 ms. |
| storm, `drop`, spec | 7.2 | 690 | 5,000 back. 50% after 3.8 s, 95% after 5.2 s, 100% after 6.7 s. No errors. |

Storms of a node, two nodes, 5,000 players, separate runs with `--scenarios storm`:

| Storm | Players hit | 50% back | 95% back | 100% back | Errors |
|---|---|---|---|---|---|
| `drop`, immediate reconnect | 5,000 | 2.3 s | 3.2 s | 3.3 s | none |
| `stop-node` (5 s drain, then 1001) | 2,500 | 10.1 s | 14.7 s | 15.3 s | none |
| `kill-node` (crash) | 2,500 | 1.7 s | 2.7 s | 2.7 s | none |

The `stop-node` time is the node's 5 s drain plus the up to 10 s spread the specification asks clients for; it is the intended shape, not slowness. Players of the dead or draining node reconnected to the other node without a failed request.

Storm on one SQLite node with `--reconnect immediate`: 100% back after 10.3 s (50% after 7.3 s), but 1,345 of the 5,000 `GET /v1/friends` calls failed to connect (the harness ran out of local ports, see below).

## What the numbers mean

- **Logins.** On SQLite the wave at 200 per second is fine on average but has a tail: p99 of a single call is 270 to 480 ms, because every sign-up is a write and SQLite has one writer; the pool of 16 was fully busy in 71% of the samples. On PostgreSQL the same wave has a p99 of 26 ms at the call and 75 ms for the whole login. A single SQLite node holds 5,000 players; it is not where a growing network stays.
- **Presence.** News is pushed straight to the connections, so delivery takes milliseconds at the median in both setups. The tail is the store: each change is a write plus a read of the friends' state, and on SQLite the tail grows when the writer is busy (p99 870 ms at 200 changes per second). Across two nodes the news goes over NATS and the p99 was still 181 ms.
- **Sessions.** Create, join and leave cost 2 to 25 ms on PostgreSQL at 30 sessions per second (about 230 calls per second, 13,000 `session` events pushed). See the bug below for what happens when many joins run at once.
- **Storms.** The cost of coming back is the work after the welcome, not the WebSocket: each returning player writes a connection row (and, when these numbers were taken, a virtual address lease, which the cloud no longer writes), and reads its friends list. With 5,000 players at once the store is the limit (the pool was busy in 31 to 95% of samples) and `GET /v1/friends` has a handler time of 340 to 1,300 ms because it waits for a store connection. The `spec` rule spreads the first retry over only one second (0.5 to 1.5 s after the break), so the whole crowd still arrives within that second; the time to be back is set by how fast the store takes them (8.75 s on SQLite, 6.7 s on two nodes with the `spec` rule; 10.3 s and 3.3 s when everyone reconnects at once).

## Where the bottleneck showed

1. **The store connection pool.** In the storm, the login wave on SQLite and the sessions run, `store connections busy` was at the limit of 16 in most samples while the CPU was far from used (1.7 cores of 20 in the wave). Handlers wait for a connection. The `/metrics` of a node has no number for that wait yet; the harness infers it from the in-process pool. A metric for pool wait and size would let an operator see this on a real cluster.
2. **The friend writes are one lane per node.** The friend graph ran at about 950 calls per second on one node and 600 on two, with 270 to 410 ms of handler time per call at a concurrency of 256, while the store was almost idle (2 to 3 of 16 connections). That is the `social` mutex of `AppState`, which serializes friend requests, accepts and blocks in a node. It works as designed and is cheap at human rates (a player sends a few friend requests a day), but a launch day or an import of friend lists runs through one lane. On two nodes it was slower, not faster: each node has its own lock and the calls are slower on PostgreSQL than on SQLite (not examined further). Fixed: see [The three bottlenecks, measured and fixed](#the-three-bottlenecks-measured-and-fixed).
3. **`GET /v1/friends` after a reconnect.** It is the most expensive call of a storm (mean handler time 340 ms to 1.3 s under 5,000 simultaneous reconnects, against 1 to 3 ms unloaded). Fixed in the same section.

## A performance bug: joins deadlock the connection pool

`POST /v1/sessions/{id}/join` holds a store transaction (one pooled connection) and, still inside it, asks the pool for a second connection: `db::blocked_between_any(&state.db, ...)` in `crates/cloud/src/routes/sessions.rs` (`join`, after `lock_session`). When as many joins run at once as the pool has connections, every one holds a connection and waits for another. Nothing can finish until sqlx's acquire timeout (30 s) fails them. In the meantime nothing else gets a connection either, so the whole node stops answering store requests: creates, leaves, sign-ins, presence.

Reproduction, on PostgreSQL with a small pool (deterministic):

```sh
anyplay-load --players 1000 --scenarios sessions --session-rate 60 --session-seconds 15 \
  --db-connections 4 --database-url postgres://postgres:test@127.0.0.1:54329/postgres
```

With 4 connections: `session_join` 6 of 2,574 succeeded, p50 30,005 ms; `session_create` p50 23 s; 2,609 requests timed out at the client; the pool was fully busy in 99% of samples while the process used 0.15 cores. The same command with `--db-connections 16` (the default): all 4,000 session calls succeed, p99 12 ms.

It also happens at the default pool size of 16 on SQLite, because SQLite's single writer makes calls overlap for longer. 1,000 players, one node, no other load:

```sh
anyplay-load --players 1000 --scenarios sessions --session-rate 30 --session-seconds 20
```

At 10 sessions per second all succeed. From 15 per second on, the node stalls for the rest of the run: at 30 per second `session_create` 143 of 599 succeeded, `session_join` 62 of 429, 709 requests timed out, 117 answered 500 (`internal`), the store error counter rose by 138, and the CPU was idle (0.1 cores). The same command on PostgreSQL with 16 connections is clean. The same stall hit the first 5,000-player run at 30 sessions per second on SQLite.

The fix is to run the block check on the transaction's connection, or before it begins, as `friends_of` and `address_of` already are. The cloud does not fix it in this change. Other handlers that call `state.db` inside an open transaction should be checked with the same method (a small `--db-connections` and a rate that overlaps).

## The three bottlenecks, measured and fixed

The friend writes, `GET /v1/friends` and the gamer cards were measured again with their fixes, against the same code without them, on one machine in one run.

Machine: AMD Ryzen 7 7735HS laptop, 16 logical cores, 30 GB, Arch Linux, nothing else running. Date 2026-10-24. Network commit `c5585ac`. Before: cloud `abe602f` (main with the cards scenario of this change and nothing else); after: cloud `400013e`. Release builds, the commands and settings of [The numbers](#the-numbers) for A and B; the cards runs add `--scenarios cards --cards-rate 200 --cards-size 10` or `100`. PostgreSQL 18 and NATS 2.15 on the same laptop, PostgreSQL's data in memory. Before and after ran back to back, one round each. The before side, measured three more times in an earlier run on the same machine, came within 25% of these numbers (the storm moved most). The raw JSON is in [`crates/load/results/2026-10-24-friends-cards/`](../crates/load/results/2026-10-24-friends-cards).

Friend graph (25,000 requests and 25,000 accepts, 256 at a time):

| Setup | Seconds | `friend_request` p50 / p99 / max | `friend_accept` p50 / p99 / max |
|---|---|---|---|
| A, SQLite, before | 21.3 | 108 / 137 / 144 ms | 109 / 137 / 144 ms |
| A, SQLite, after | **13.5** | 69 / 78 / 99 ms | 69 / 78 / 82 ms |
| B, two nodes, PostgreSQL, before | 49.2 | 248 / 386 / 398 ms | 249 / 385 / 398 ms |
| B, two nodes, PostgreSQL, after | **16.6** | 117 / 208 / 246 ms | 46 / 89 / 121 ms |

`GET /v1/friends` in the storm (`drop`, spec reconnect, 5,000 players):

| Setup | Answered when the storm ended | p50 | p99 |
|---|---|---|---|
| A, SQLite, before | 5,000 | 701 ms | 1,152 ms |
| A, SQLite, after | 5,000 | **152 ms** | 320 ms |
| B, two nodes, PostgreSQL, before | 3,496 (the rest still waiting) | 1,036 ms | 1,219 ms |
| B, two nodes, PostgreSQL, after | 5,000 | **267 ms** | 361 ms |

The storm ends when every player is back; each fetches its lists after the welcome, so a list still waiting then is not counted. The time until all players were back (2.0 s on A, 2.1 to 2.2 s on B) did not change: it is set by the reconnect rule.

Gamer cards, 200 calls per second for 30 s:

| Setup | Cards per call | Succeeded | p50 | p99 |
|---|---|---|---|---|
| A, SQLite, before | 10 | 5,998 of 5,998 | 3.0 ms | 3.5 ms |
| A, SQLite, after | 10 | 5,998 of 5,998 | 1.1 ms | 1.7 ms |
| A, SQLite, before | 100 | 5,999 of 5,999 | 15.1 ms | 16.4 ms |
| A, SQLite, after | 100 | 5,999 of 5,999 | **1.9 ms** | 3.5 ms |
| B, two nodes, PostgreSQL, before | 10 | 5,999 of 5,999 | 13.6 ms | 15.2 ms |
| B, two nodes, PostgreSQL, after | 10 | 5,999 of 5,999 | 3.0 ms | 3.8 ms |
| B, two nodes, PostgreSQL, before | 100 | **996 of 5,495** | 30,001 ms | 30,002 ms |
| B, two nodes, PostgreSQL, after | 100 | 5,999 of 5,999 | **8.9 ms** | 10.8 ms |

Before, 100 cards per call on PostgreSQL kept all 16 store connections busy (15.9 on average) 4,499 calls ended at the client's 30 s timeout, and the harness skipped 504 more it had no room for. After, 0.9 connections were busy on average.

### What changed

- **Friend writes hold the two accounts, not one lock per node.** A request, accept, cancel, unfriend, block or unblock runs in one store transaction that, on PostgreSQL, locks the rows of the two accounts in the order of their IDs. Changes between other accounts go on at the same time, and the limits (friends, open requests, blocks, a new account's daily requests) count what every node has written. The lock per node did not: two nodes let a new account send 11 requests of an allowance of 10 in 2 of 5 runs of a test that sends 20 at once through both; with the change 10 of 10 runs keep to 10.
- **SQLite takes its writers in order.** SQLite lets one writer in at a time and makes the others retry after sleeps that grow the longer they have waited, so later writers win over earlier ones. Without the lock per node, and before this, the slowest friend request on SQLite took 1.1 to 1.7 s against a p99 of 0.2 s. Write transactions on SQLite now wait in a queue of the node and reach SQLite in the order they asked; a store test of twelve waiting writers fails 10 of 10 times without the queue.
- **`GET /v1/friends` reads everything on one store connection.** It asked the pool for a connection five times (friends, both request lists, blocks, presence), and in a storm each ask waits.
- **Gamer cards in a fixed number of queries.** `GET /v1/cards` read each account, its relation to the viewer, its privacy, trophies and avatar one by one: up to six queries per card, each on a connection of its own, for up to 100 cards per call. It now reads each part for all the cards at once, on one connection: at most seven queries per call.

What is left: on PostgreSQL the friend graph now keeps all 16 store connections busy (16.0 on average, 1.5 before), so the pool is its limit, as it already was for the storm (item 1 below).

## What the harness did to itself

- **Local ports.** A storm of 5,000 players opens 5,000 WebSockets and, at once, 5,000 new HTTP connections for the friends list. On Windows the harness then ran out of ephemeral ports (`WSAEADDRINUSE`): 1,345 failed requests in the immediate storm on SQLite, 8 in the spec storm. These failures are the harness's and the operating system's, not the cloud's; they are counted as `http_error:connect` in the report. They cap what one machine and one source IP can play at about 5,000 players.
- **Shared cores.** The harness, both nodes, PostgreSQL and NATS share 20 cores. The CPU numbers cannot be split between harness and cloud in process mode.
- **One clock.** Delivery latency is measured by one process with one clock, so it is exact, but it includes the harness's own scheduling (a few milliseconds at the median).
- **Missing events.** 10 of 59,980 presence events did not arrive within 3 s in the cluster run (none in the single-node runs). It was not investigated: the cloud tells friends the latest state, so two changes of one player a few milliseconds apart may be merged into one event, which would make this a measurement artifact, but the harness cannot tell a merged event from a lost one.

## What these numbers do not show yet

- A **real cluster**: separate machines, a network between the nodes, PostgreSQL with replicas, NATS with its own cores, a load balancer with TLS. Everything here crossed the loopback interface, and the databases shared the laptop with the players.
- **Many source IPs.** The per-IP limits could not be used, so the in-process cloud runs with them lifted (`--default-limits` keeps them and shows the 429s). Rate limiting under a real login wave is untested, as is the per-node counting of limits.
- **Scale beyond 5,000 players**, long runs (hours: leaks, sweeps, the 15-minute token lifetime, 20-second pings under load), and a mixed day: this was five scenarios of 30 s each.
- **Real clients.** Players here have no think time, never send chat or invitations, never block each other, and reconnect exactly by the rules.
- **Cloud-only memory and CPU.** Run the cloud as its own process and pass `--pid`; the external mode has the harness side for it, the rate limits are what stops a large run.
- **Slow networks.** Latency here is the cost of the cloud's own work; a mobile network adds to every number.
