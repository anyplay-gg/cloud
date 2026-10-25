# What the cloud does

Everything in the [cloud API](https://github.com/anyplaynet/network/blob/main/spec/cloud-api.md) works and is tested end to end. The API may still change while the version is 0.

| Service | What players get | State |
|---|---|---|
| Accounts | Sign-in without a password: a code by e-mail, then a refresh token per device that changes on every use. Export and deletion of all data. | works |
| Friends and blocks | Requests by handle, mutual requests become friends at once, blocks stay private. | works |
| Presence | Who is online, what they play, and whether their game is open to join. | works |
| Sessions | Create, join and leave, with capacity, a leader, opaque game properties and a version guard: only the same game version and AnyPS5 build play together. | works |
| Invitations | Invite friends into a session; the invitation ends with the session. | works |
| Join codes | A short code that stands for a player and lets someone who is not a friend, a Discord friend for example, ask to join the game the player offers. Redeeming one makes an ordinary invitation, and nothing joins by itself. The cloud keeps only a hash of the code; a code lives 2 hours by default (`join_code_minutes`), and a player has one at a time. | works |
| Trophies | Records of earned trophies (title, number, grade, time; no names or art), rarity once 10 players own a title, gamer cards with counts by grade. Private by default: friends see them, and each player can show them to everyone or nobody. | works |
| Profiles | A short text (160 characters, no links), up to 3 favourite titles, an avatar and playtime per title, each with its own visibility (friends by default, everyone or nobody). Avatars are PNG or JPEG up to 1 MB, decoded with limits against decompression bombs, cropped square, scaled to 256 by 256 and stored again as PNG without metadata, in the database, served by content hash with long cache headers; 5 uploads per account and hour. Playtime keeps the highest minutes seen per title and day, so uploads can repeat. | works |
| Profile page and feed | `GET /v1/accounts/{id}/profile` also answers the trophy counts and the connections the caller may see. The feed lists the player's own and their friends' recent trophies (with grade and rarity), new friends, first plays of a game and parties (only members see those), newest first and paged. Items hold numbers and kinds, never names or text. Four fixed reactions, one per account and item, no comments. Visibility is the player's `feed` setting (friends by default; `nobody` also stops recording). At most 100 items per account and day, kept 90 days (`feed_retention_days`); 60 reactions per account and minute. | works |
| Relays | Signed tickets that let one endpoint use one relay for 24 hours. Relays check them offline and never call the cloud. | works |
| Moderation | Reports, bans per account and e-mail address, rate limits on every route, and an admin command line. | works |
| Parties and messages | Parties with a leader and members, and text messages in one-to-one and group conversations. A message from someone who is not a friend waits in the receiver's message requests. Voice and calls run between players, not through the cloud. | works |
| Matchmaking, leaderboards, save backups | | planned |

## How it is built

- **Requests and events.** The net service calls a small JSON API over HTTPS and holds one WebSocket that pushes events: a friend's presence, an invitation, a change in a session. Events are short, and the client reads the full state after each reconnect, so a lost event never leaves it wrong.
- **Tokens.** The cloud signs its own tokens with Ed25519 ([`crates/ticket`](../crates/ticket)): access tokens that live 15 minutes, and relay tickets bound to one endpoint. A refresh token used twice signs the device out, because only a thief would do that.
- **Relays.** [iroh](https://www.iroh.computer/) relays carry encrypted QUIC traffic between players and cannot read it. `anyplay-relay` admits an endpoint only with a ticket the cloud signed for it.
- **One program, two shapes.** Self-hosters run one process with a SQLite file. AnyPlay's own cloud will run the same program on many nodes with PostgreSQL and NATS, and the same tests run on both ([ADR 0002](adr/0002-one-program-two-shapes.md)). The road from one process to a cluster is in [ARCHITECTURE.md](ARCHITECTURE.md).

## What the cloud never does

It never sees game files, never contacts a platform holder's servers and never stands in for a publisher service that still runs. [The line is drawn at the law](https://github.com/anyplaynet/network/blob/main/docs/adr/0021-the-line-is-drawn-at-the-law.md) explains where the network's line lies. Revival servers for services a publisher has shut down are allowed and not built yet.
