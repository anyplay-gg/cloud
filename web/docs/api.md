# What the portal expects from the cloud

The client talks JSON to `/web/api/v1` on its own origin, with the session cookie (`credentials: same-origin`) and `X-CSRF-Token` on every write. This file is the contract the client was written against. The web API PR (`network/spec/web-api`) is the authority; where it differs, change `src/api/http-adapter.ts` and this file, and nothing else. Today the cloud serves HTML under `/web/*`; none of these JSON routes exist yet.

Keys are `snake_case` on the wire and `camelCase` in the app (`src/api/case.ts`). Times are ISO 8601 UTC.

## Errors

Non-2xx answers carry `{ "error": { "code": string, "message": string, "fields"?: {name: message}, "methods"?: ["passkey","email_code"], "retry_after"?: seconds } }`.

| Status | Client treats it as |
|---|---|
| network failure | `network` |
| 401 | `unauthorized` (the session ended; the guards send the person to sign in) |
| 403 with `code: "step_up_required"` and `methods` | `step_up_required`: the step-up dialog, then the same write again |
| 403 | `forbidden` |
| 404 / 409 | `not_found` / `conflict` |
| 400 / 422 with `fields` | `invalid`: each field message is shown under its input |
| 429 with `retry_after` | `rate_limited` |
| 5xx | `server` |

## Routes

| Method and path | Body / answer |
|---|---|
| `GET /session` | `{ signed_in, csrf, terms: {url, version}, account: Me or null }`. Never fails for a signed-out visitor. |
| `POST /auth/email` | `{ email, intent: "sign_in" or "create" }` answers `{ email_hint, resend_after }`. Same answer whether the address is known. |
| `POST /auth/code`, `/auth/code/resend` | `{ code }` answers `{ next: "code" or "details" or "terms" or "done" }`; resend answers like `/auth/email` |
| `POST /auth/details`, `/auth/terms` | `{ handle, display_name }`, `{ accept: true }`; both answer `{ next }` |
| `POST /auth/passkey/options`, `/auth/passkey/verify` | `{ options, id }` (WebAuthn request options as JSON), then `{ id, credential }`; answers `{ next }` |
| `POST /auth/sign-out` | 204 |
| `PUT /account/profile` | `{ display_name, about, accent, theme }` answers `Me` |
| `GET /account/activity` | `[ { id, kind, at, detail } ]` |
| `GET /account/passkeys` | `[ { id, name, created_at, last_used_at, synced } ]` |
| `POST /account/passkeys/options`, `POST /account/passkeys` | `{ options, id }`, then `{ id, name, credential }` answers the passkey. Needs a recent sign-in. |
| `DELETE /account/passkeys/{id}` | Needs a recent sign-in. |
| `GET /account/sessions`, `DELETE /account/sessions/{id}`, `POST /account/sessions/sign-out-everywhere` | Devices and browsers; the last needs a recent sign-in. |
| `GET /account/step-up` | `{ methods }` |
| `POST /account/step-up/passkey/options`, `/passkey/verify` | as for sign-in |
| `POST /account/step-up/email`, `/email/verify` | `{ code }`; the first answers like `/auth/email` |
| `GET` / `PUT /account/privacy` | `{ fields: {about, friends, trophies, activity: "everyone" or "friends" or "me"}, friend_requests: "everyone" or "friends_of_friends" or "nobody" }` |
| `GET /account/export` | The JSON file as an attachment. Needs a recent sign-in. |
| `POST /account/delete`, `POST /account/delete/cancel` | `{ handle }` starts the 30-day grace period and answers `Me` with `deletion: {requested_at, scheduled_for}`. Needs a recent sign-in. |
| `GET /friends` | `{ friends, incoming, outgoing }` |
| `POST /friends/requests` | `{ handle }` |
| `POST /friends/requests/{id}/accept`, `/decline`; `DELETE /friends/requests/{id}`; `DELETE /friends/{id}` | |
| `GET /trophies` | `{ days: [{date, count}], titles: [{title_id, name, earned, total, percent, last_earned_at}], earned, total, percent }`. Counts only: never a trophy name, text or art. |
| `GET /profiles/{handle}` | `PublicProfile`, already filtered for the viewer: fields the viewer may not see are `null` and listed in `hidden` with the audience that could. 404 for an unknown handle. |

`Me`, `PublicProfile` and the rest are typed in `src/api/types.ts`.

## Assumptions to confirm

- Handles follow the cloud's current rule: 3 to 16 letters, digits, hyphens and underscores, starting with a letter.
- Deletion copy ("your profile disappears", "sign in within 30 days to cancel") describes the grace period the brief asked for; confirm it matches what the cloud does.
- The launcher hand-off ends on `/return` after the cloud has signed the browser in; the app does not call the launcher.
- Title names come from the launcher (text only), and `days` is one count of trophies per day.
