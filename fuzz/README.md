# Fuzz targets

Coverage-guided fuzzing with [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) of the token check in `crates/ticket`. The cloud runs it on every request and the relay on every endpoint that connects, so it reads whatever any client sends.

| Target | What it reads | What must hold |
|---|---|---|
| `ticket_forged` | any text presented as a token | refused, as either kind, at any time |
| `ticket_altered` | a token the cloud signed (version 2 access and relay, and a legacy version 1 access token), with bytes overwritten | it passes only as it was issued, with the claims it was issued with |

Both sign with a fixed key that exists only in the targets. The tests in `crates/ticket` cover the same cases by hand on every `cargo test`; these targets search further.

## Run

cargo-fuzz needs a nightly toolchain and Linux or macOS. Clone `network` next to `cloud`, as for any build:

```sh
cargo install cargo-fuzz --version 0.13.2 --locked
cargo +nightly fuzz run ticket_altered -- -max_total_time=300
```

An input that breaks a target is saved under `fuzz/artifacts/<target>/`. Replay it with `cargo +nightly fuzz run <target> <file>`, then add it to the tests in `crates/ticket` as a fixed case before fixing the check.

## In CI

[`.github/workflows/fuzz.yml`](../.github/workflows/fuzz.yml) runs each target for a minute on pull requests that touch the token check, and for a quarter of an hour every night. The corpus grows from run to run in the Actions cache. When a target fails, the input is attached to the run as the artifact `crash-<target>`.
