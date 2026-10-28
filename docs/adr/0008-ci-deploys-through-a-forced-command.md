# CI deploys through a forced command

[ADR 0007](0007-images-are-built-in-ci-and-pulled.md) made a deploy a pull, but someone still had to log in and run `update.sh`. A merge to the website's `main` is now live in a few minutes without a hand on the server, and a cloud deploy is one `gh workflow run`.

## Decision

The deploy is a single `ssh` call from a GitHub Actions job to the server, with a key that can do nothing else:

- **A restricted key.** The public half is in `/root/.ssh/authorized_keys` as `restrict,command="/usr/local/sbin/anyplay-deploy",no-pty,no-port-forwarding,no-agent-forwarding,no-X11-forwarding`. The private half is the secret `DEPLOY_SSH_KEY` in `anyplaynet/web` and `anyplaynet/cloud`. The server's host key is pinned in the variable `DEPLOY_HOST_KEY` (`StrictHostKeyChecking=yes`).
- **A wrapper that takes two requests.** [`deploy/anyplay-deploy`](../../deploy/anyplay-deploy) is copied to `/usr/local/sbin` (root-owned, so a checkout cannot change it). It accepts `web sha256:<64 hex>` or `cloud <40 hex commit> sha256:<64 hex> sha256:<64 hex> [sha256:<64 hex>]` and refuses everything else, multi-line input included. Only strings that matched those patterns are ever used.
- **Images from our CI only.** Before it pulls anything, the wrapper runs `gh attestation verify oci://ghcr.io/anyplaynet/<image>@<digest> --bundle-from-oci` with `--repo`, `--signer-workflow` (`docker.yml` of the web repository, `image.yml` of this one) and `--source-ref refs/heads/main`; for the cloud and relay it also requires `--source-digest <commit>`. A digest that was not built by that workflow on `main` is refused. `--bundle-from-oci` reads the attestation from the registry, so the server holds no GitHub token (`gh` only wants the variable to exist).
- **A web deploy touches only the web container** (`update.sh --web-only`): no checkout, no build, no backup; the cloud, relay and Caddy keep running. It keeps its rollback: the previous image is tagged first and put back if the new container is not healthy or the public address stops answering.
- **A cloud deploy** is the existing `update.sh --image` with the backup and the 30 s of downtime. It stays on demand (`deploy.yml`, `workflow_dispatch`) and keeps the website image that is running.

## What a leaked key can do

It can start a deploy of an image that our workflows built on `main` and attested: an older web build or an older cloud build (a downgrade; a cloud deploy also costs a backup and 30 s of downtime), over and over. That is a nuisance and a way to roll the site back to something old, not a way to run other code.

It cannot get a shell, read or copy a file, forward a port, run any command other than the two, or deploy an image that those two workflows did not attest on `main`. It cannot change the wrapper or `authorized_keys`.

Not covered: anyone who can merge to `main` of `anyplaynet/web` or `anyplaynet/cloud`, or change the workflow files there, can build an image that passes the check. That is the trust ADR 0007 already names; branch protection is the control, and a stolen key does not widen it. To revoke, delete the `authorized_keys` line and the two secrets; to rotate, make a new pair.

## Consequences

- `gh` (a pinned release, checksum-verified) is installed on the server at `/usr/local/bin/gh`.
- Every request is logged to `/var/log/anyplay-deploy.log` (time, client, the request); refused ones too.
- The first web deploy needs a server whose `update.sh` knows `--web-only`; the wrapper says so and stops if it does not.
