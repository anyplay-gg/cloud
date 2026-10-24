# NATS carries news, not state

Nodes of a cluster tell each other which accounts have news over NATS core subjects, one per account (`anyplay.account.<id>`). A note may be lost, because the store holds the truth: events that carry an object carry its revision, events without one make the client read, and every new event connection starts with a fresh read. JetStream and the NATS key-value store hold no state of the cloud. Jepsen's analysis of NATS 2.12.1 (December 2025) found acknowledged JetStream writes lost under file corruption and a default that writes to disk only about every two minutes; core subjects ask for no such guarantee. NATS joins regions through gateways and leaf nodes, which the cells of a later stage need.

## Considered options

- Valkey or Redis publish and subscribe: as good for one region, with no topology for several. Redis 8 is offered under RSALv2, SSPLv1 or AGPLv3; Valkey is BSD-3.
- Kafka or Redpanda: a durable log, which news that may be lost does not need, at the cost of the heaviest system to run of all.
- Postgres `LISTEN` and `NOTIFY`: no extra service, but every note passes through the primary, replicas never see them, payloads stop at 8,000 bytes, and each node needs a connection of its own that listens on a channel per connected account.
