# Postgres for a cluster

A cluster keeps its data in PostgreSQL, run with the CloudNativePG operator (failover, replicas, backups with point-in-time recovery, standby clusters in other regions); a single process keeps its SQLite file ([ADR 0001](0001-sqlite.md)). Both run the same SQL through sqlx's `Any` driver. The queries use what both databases understand, are written once with `$1` parameters (turned into `?1` for SQLite), and only the migrations exist twice. The behaviour tests run on both.

## Considered options

- CockroachDB: since November 2024 self-hosting needs an enterprise licence, free only below a revenue limit and with telemetry. An AGPL cloud that anyone may run cannot depend on it.
- YugabyteDB, TiDB and other distributed SQL databases: they solve writes spread over many regions, which this network does not need. Accounts and friends are small and written rarely; one Postgres primary per region carries them, with read replicas where reads grow.
- A store interface with a hand-written SQLite and Postgres implementation each: twice the SQL to keep in step, and the two would start to behave differently. The types the `Any` driver lacks (JSON, dates) are types this schema does not use: it stores integers, text and bytes.
