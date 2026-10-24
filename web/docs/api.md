# The portal's API

The contract is the network's [`spec/portal-api.md`](https://github.com/anyplay-gg/network/blob/main/spec/portal-api.md) (anyplay-gg/network#23), implemented by the cloud in `crates/cloud/src/web/api` (cloud#29 for sign-in, account and serving, cloud#30 for friends, profile, trophies and public profiles). This file only says where the client lives and what it does on top.

## Where it lives

- `src/api/portal-api.ts` is the interface the screens use. `src/api/http-adapter.ts` implements it against `/web/api/v1` (same origin, `credentials: "same-origin"`); `src/api/mock/` implements it in memory for `pnpm dev` and the screenshots.
- Keys are `snake_case` on the wire and `camelCase` in the app (`src/api/case.ts`). Times are Unix seconds; ids are integers.

## What the client does for the API's rules

| Rule in the spec | In the client |
|---|---|
| Every write is JSON, even with no body | Sends `Content-Type: application/json` and `{}` on every write. The avatar is the exception: it sends the picture with its own `image/png` or `image/jpeg`. |
| `X-CSRF-Token` from `GET /session`, new with every new session | Kept in the adapter; asked for on the first write; replaced by the `csrf` of every sign-in step; dropped on sign-out. |
| `error` is a code, `field` names an input | `ApiError.code` is the cloud's code; `fields[field]` carries its message to the input. `Retry-After` becomes `retryAfter`. An HTML body (the origin guard's refusal) is read as `unknown`/status only. |
| `403 step_up_required` | `useStepUp().run(write)` opens the confirm dialog, then runs the write again. The methods offered are the account's: a passkey if it has one, the e-mail code always. The dialog uses `POST /step-up/email` + `/step-up/code`, or `POST /passkey/sign-in/start` + `/step-up/passkey/finish`. |
| Every sign-in step answers with the session view; `flow.step` says what is next | `AuthScreen` shows `flow.step` and keeps no flow state of its own. `done` goes to the account, `banned` shows the ban statement and appeal in place, `handoff` shows "You can return to AnyPlay" and calls `location.assign(flow.redirect)`. |
| A part a viewer may not see is `null` | Public profiles render only what is there and say that friends may see more. |

## What the API does not have, and what the portal does about it

- **No display name, accent or banner theme.** A player is `@handle`. The avatar colour and the banner gradient are derived from the handle (`src/lib/identity.ts`), so a player looks the same everywhere without storing anything. The accent in the Appearance menu is a setting of this browser.
- **No trophy totals or names.** The portal shows counts per grade and per title id, and the share of titles, never a percentage and never a trophy or game name.
- **No activity log, no per-day trophy data.** The overview lists the launchers and browsers used most recently instead; the heat map was left out.
- **Browsers have no name.** A browser session is "This browser" or "Another browser", with the day it signed in.

## Routes the cloud serves the shell for

`/`, `/sign-in`, `/create`, `/account` and below, `/friends` and below, `/trophies` and below, `/u/{name}`. The portal's routes are exactly these (`src/routes`): the ban statement and the launcher hand-off live inside `/sign-in`, not on routes of their own, so a reload never lands on a path the cloud does not know.

## Still open

- Title names: the cloud has title ids only, so games show as ids (`GAME01234`) until the launcher can supply names.
- E-mail notices for security changes are not sent (the spec lists it as open); the portal does not promise them.
