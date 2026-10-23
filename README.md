# AnyPlay Cloud

The central services of the AnyPlay network: accounts, friends, presence, sessions, matchmaking, invitations, signaling and relays.

**Status: planned.** Work starts in phase 1 of the network's [roadmap](https://github.com/anyplay-gg/network/blob/main/docs/DESIGN.md#roadmap). The design and the words we use are in [anyplay-gg/network](https://github.com/anyplay-gg/network).

## Hosting your own

One `docker compose up` will start a complete cloud with a relay, and the launcher will let you point at any cloud. No player has to depend on a single operator.

## What the cloud never does

It never sees game files, never contacts Sony's servers and never stands in for a publisher's servers. The [clean line](https://github.com/anyplay-gg/network/blob/main/docs/adr/0003-the-clean-line.md) explains why.

## License

AGPL-3.0. If you run a modified cloud for other people, you must publish your changes.
