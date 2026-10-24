# AnyPlay web portal

The account portal for https://anyplaynet.com: sign in, security, privacy, data, profile, public profiles, friends and trophies, plus a signed-out landing page. A static React app (Vite, React 19, TanStack Router and Query, Tailwind 4, Base UI, Motion). No server of its own: the build is files the cloud serves on the same origin as `/web/api/v1`.

## Run

Needs Node 22 or newer and pnpm 12 (`packageManager` pins it).

```sh
pnpm install --frozen-lockfile
pnpm dev            # http://localhost:5173, against the in-memory mock, no backend needed
pnpm dev:cloud      # the same, with /web/api proxied to a cloud (ANYPLAY_CLOUD_URL, default http://127.0.0.1:8080)
```

The mock has states: add `?mock=empty`, `?mock=error`, `?mock=slow` (everything loading) or `?mock=signedout` to any URL. It keeps the choice for the tab.

## Check and build

```sh
pnpm typecheck      # compiles the messages, then tsc
pnpm lint           # Biome
pnpm test           # Vitest + Testing Library; includes axe on every screen
pnpm build          # dist/, the real bundle (talks to /web/api/v1, no mock code in it)
pnpm build:mock     # dist-mock/, the same app with the mock, for screenshots
pnpm a11y           # axe in the installed Chrome, contrast included, every screen, dark and light, desktop and phone
pnpm shots [dir]    # screenshots of every screen (not part of the repo)
pnpm tokens         # regenerate the colour tokens and docs/colour.md
```

`pnpm a11y` and `pnpm shots` drive the Chrome at `C:/Program Files/Google/Chrome/Application/chrome.exe` (set `CHROME_PATH` for another); nothing is downloaded.

## Shape

```
src/api/       the one seam: PortalApi, an HTTP adapter, an in-memory mock, typed errors, query hooks
src/ui/        primitives on Base UI in the portal's look
src/components/ shell, layout, and the pieces more than one screen uses
src/screens/   one file per screen; src/routes/ are thin file routes
src/styles/    tokens.generated.css (from scripts/gen-tokens.mjs), tokens.css (names), index.css
messages/      every visible string (Paraglide JS, English only for now)
docs/          colour.md (measured contrast), api.md (what the client expects from the cloud)
```

## Rules this app keeps

- **Colour** comes from `scripts/gen-tokens.mjs`: a gray with no tint, the launcher's six accents, status and trophy grades, each a 12-step ramp, dark and light as separate palettes, every text, border and focus pair measured (`docs/colour.md`). Primary buttons are ink, not accent; colour marks progress, selection, focus and the online dot.
- **CSP** is `script-src 'self'; style-src 'self'; font-src 'self'`: no inline scripts or styles, no CDN. Fonts are self-hosted, `index.html` has no inline code, the theme is applied by `public/theme-init.js`, and Base UI runs with `disableStyleElements`.
- **Motion** is `transform` and `opacity` under 300 ms, press feedback is `scale(0.97)`, nothing animates on a keyboard-repeated action, and `prefers-reduced-motion` turns movement into fades.
- **Clean line**: no game art, no trophy names, texts or art (the cloud never has them), no platform names. Trophies are counts and progress per title.
- **Supply chain**: exact versions, a lockfile, `minimumReleaseAge` of seven days, `trustPolicy: no-downgrade`, `blockExoticSubdeps`, build scripts off (`pnpm-workspace.yaml`).

Borrowed patterns and their licences: `THIRD_PARTY.md`.
