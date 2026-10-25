# Host your own cloud

You need a machine with Docker, two host names that point at it (one for the cloud, one for the relay), an SMTP account that can send mail, and these ports open: 80, 443 and 8443 over TCP, and 7842 over UDP.

The build needs the network repository next to this one, because the cloud uses its API crate:

```text
workspace/
  cloud/       this repository
  network/     anyplaynet/network
```

The cloud is built and deployed against one network revision, the commit in [`network.ref`](../network.ref). CI, the Docker build in `deploy/update.sh` and the nightly jobs all read that file, so a change on the network's `main` cannot break this repository by itself. Check out that commit in `network/` to build as CI does. A pull request that needs new network code (a new API field, a new crate, a changed `Cargo.lock` entry) bumps `network.ref` in the same pull request; the workflow `network-drift` shows early when `main` of the network has moved on in a way that needs a bump.

Then:

```sh
cd cloud/deploy
cp .env.example .env     # set the two host names, your e-mail address and the SMTP details
docker compose up -d
```

Caddy gets certificates and puts TLS in front of the cloud. The cloud creates its signing key in its data volume on the first start and writes the public half next to it; the relay waits for that file and for Caddy's certificate, then starts. Point a launcher at `https://<your cloud host name>`.

For a first try on your own machine, set `MAIL=log` in `.env`: sign-in codes then appear in `docker compose logs cloud`.

## Operate it

- **Update.** Use [`deploy/update.sh`](../deploy/update.sh) ([deploy.md](deploy.md)): it backs up the data volume, builds, replaces the containers, checks the public address and rolls back on failure. Restoring a backup: [deploy.md](deploy.md#backups).
- **Back up** the `cloud-data` volume. It holds the database and the signing key. Losing the key signs everybody out and makes every relay refuse its players until it is given the new public key.
- **Moderate.** With `ADMIN_TOKEN` set in `.env`, `docker compose exec cloud anyplay-cloud admin reports` lists reports, and `admin ban` and `admin unban` act on a handle. All commands are in [moderation.md](moderation.md).
- **Check the mail.** `docker compose exec cloud anyplay-cloud admin mail-test someone@example.com` sends one test mail through the running cloud. It prints `sent`, or `failed: <class> (SMTP <status>)`; the classes are listed in [configuration.md](configuration.md#sign-in-mails).
- **Settings.** Every setting is described in [configuration.md](configuration.md). That page also says how to run the relay on a machine of its own with a Let's Encrypt certificate.
- **Cluster.** To run the cloud on Kubernetes, see [kubernetes.md](kubernetes.md).

## Develop

```sh
cargo run -p anyplay-cloud       # http://127.0.0.1:8080, sign-in codes in the log, no TLS
cargo test --workspace
```

Without settings the cloud runs in development mode: codes go to the log, nothing is encrypted and anyone can sign up. `anyplay-cloud keygen` prints the public key a relay needs. The cloud finds the API crate at `../network/crates/api`, so clone both repositories side by side.

Every pull request runs formatting, Clippy with warnings as errors and all tests on Windows and Linux, builds the three images and starts them with the compose file. Both fuzz targets of the token check run for 15 minutes each every night.
