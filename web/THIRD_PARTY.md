# Third-party pieces

The portal is AGPL-3.0. Everything it borrows is MIT, ISC, Apache-2.0 or OFL, and nothing is copied from a library whose licence is a Commons Clause, a paid "Pro" tier or an attribution-only licence (Canvas UI, Rare UI, Sora UI and every "Pro" bundle are not used).

## Runtime dependencies and fonts

Resolved from `pnpm-lock.yaml`. The ones that shape the product:

| Package | Licence | Used for |
|---|---|---|
| `react`, `react-dom` | MIT | UI |
| `@base-ui/react` | MIT | Accessible primitives: dialog, alert dialog, menu, select, tabs, tooltip, toast, switch, checkbox, field, OTP field, preview card, radio group, avatar, progress |
| `@tanstack/react-router`, `@tanstack/react-query` | MIT | Routing with typed search params, server state |
| `motion` | MIT | `LazyMotion` + `m` for the few transitions that need springs or exit animations |
| `lucide-react` | ISC | Icons |
| `react-hook-form`, `@hookform/resolvers`, `zod` | MIT | Forms and validation |
| `tailwindcss`, `clsx`, `tailwind-merge` | MIT | Styling |
| `@inlang/paraglide-js`, `@inlang/plugin-message-format` | Apache-2.0 | i18n compiler (English only for now) |
| `@fontsource-variable/inter` | OFL-1.1 (font), MIT (package) | Inter Variable with the optical-size axis |
| `@fontsource-variable/geist-mono` | OFL-1.1 (font), MIT (package) | Geist Mono for codes and IDs |

Sonner was the first choice for toasts and is not used: it injects a `<style>` element at runtime and the CSP is `style-src 'self'`. Base UI's toast does the same job.

## Borrowed UI patterns

Each of these is a pattern that a public MIT library shows in its docs and demos, **written again** from its published behaviour on Base UI, Tailwind and the portal's tokens. The libraries' licence files and head commits were checked on 2026-10-24; their source files were not copied, so no file carries a third-party header. The notices are kept below anyway because the designs are theirs, and so that the next person knows where to look.

| Piece in the portal | Source | Licence | Commit checked | What the portal changed |
|---|---|---|---|---|
| Magnetic dock for the links on the landing (`src/components/magnetic-dock.tsx`) | Componentry, https://github.com/harshjdhv/componentry (https://componentry.dev) | MIT | pattern only, repository listing checked | Spring-scaled icons toward a fine pointer, off for touch, keyboard and reduced motion; every item stays a link with a name; Base UI tooltip |
| Launcher window mockup with neutral tiles (`src/components/launcher-mockup.tsx`) | Cult UI device mockups, https://github.com/nolly-studio/cult-ui | MIT | `7f031dc812f7bda10213bab62752dd6d080297ff` | Drawn from the launcher's own chrome (docs/DESIGN.md); grey placeholder tiles, no art, hidden from assistive technology |
| Marker stroke behind one headline word (`.marker` in `src/styles/index.css`) | Highlighters, https://github.com/JaceThings/Highlighters | MIT | pattern only | A pseudo-element scaled in with `transform`; static under reduced motion |
| Layered soft shadows and concentric radii (`LauncherMockup`, `--shadow-*`) | movements.dev | pattern only | | Three-layer shadows; 20 = 12 + 8 radius rule |
| One-time-code input, error line and shake (`src/ui/otp-input.tsx`, `.shake`, `[data-field-error]`) | Bencho, https://github.com/cabralorenzo/bencho | MIT | `91d66c97d2dc2cbef0089b220439eb41705f1303` | Built on Base UI's OTP field; the shake is `transform` only and off for reduced motion |
| Halo focus ring on text controls (`.halo`) | Cult UI, https://github.com/nolly-studio/cult-ui | MIT | `7f031dc812f7bda10213bab62752dd6d080297ff` | Soft accent ring on top of the solid 2px outline, which stays; measured in `docs/colour.md` |
| Active sessions list (`DevicesSection` in `src/screens/security.tsx`) | ObsidianUI, https://github.com/Atharvsinh-codez/ObsidianUI | MIT | pattern only, repository listing checked | The current session set apart, the rest listed below; sign-out held back for six seconds with Undo |
| Confirm morph (`src/ui/confirm-morph.tsx`) | Arc UI, https://github.com/kuratlielia/arc-library | MIT | `fff39c044c6e3b86006e3a6acc9a32a821a80d99` | Rewritten with Base UI's button and Motion; focus lands on Cancel, Escape backs out, blur cross-fade |
| Undo toast (`src/lib/use-undoable.ts`, `src/ui/toast.tsx`) | Arc UI (undo), beUI https://github.com/starc007/ui-components | MIT | `fff39c0…`, `8fd73d5f39edd09f11ebf6dfbb4992dcb85f8a7b` | Delayed commit instead of a reversible write; the stacked toast is Base UI's |
| Stacked toasts (`.toast` in `src/styles/index.css`) | beUI `animated-toast-stack`, https://github.com/starc007/ui-components | MIT | `8fd73d5f39edd09f11ebf6dfbb4992dcb85f8a7b` | Base UI's toast with the stack and peek CSS; slides on `transform`/`opacity` |
| Copy field (`src/ui/copy-field.tsx`) | Bencho | MIT | `91d66c97d2dc2cbef0089b220439eb41705f1303` | Icon swap with scale, opacity and blur, bounce 0; a toast says it was copied |
| Skeleton morph (`.fade-in` in `QueryView`) | Bencho | MIT | same | Loaded content fades in over the skeleton's place |
| Rolling counter (`src/ui/rolling-number.tsx`) | Bencho | MIT | same | Digit strips moved with `transform`; the plain number is the accessible name |
| Profile hover card (`src/components/person-card.tsx`) | Bencho, built on Base UI's preview card | MIT | same | Shows only what the list already has |
| Avatar group (`AvatarGroup` in `src/ui/avatar.tsx`) | Bencho | MIT | same | One `role="img"` with one description |
| Activity heat map (`src/components/heatmap.tsx`) | Bencho and Markdown Graphs, https://mdx-graphs.kshv.me | Bencho MIT; Markdown Graphs licence **could not be verified** | Bencho `91d66c9…` | Written from the pattern alone, nothing copied from Markdown Graphs; counts days only |

## Licence notices kept

The MIT licences of the repositories above read, in full, as follows. The copyright lines are the ones in each repository's `LICENSE` file on the commit named above.

> MIT License
>
> Copyright (c) 2026 Lorenzo Cabra (Bencho)
> Copyright (c) the Cult UI, Arc UI, beUI, ObsidianUI, Componentry and Highlighters authors, as stated in each repository's `LICENSE`
>
> Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

If a file is ever copied from one of these repositories instead of written again, add its header, its row here with the file name, and the exact copyright line from that repository.

## Pieces dropped

- **Cult UI "Halo" as a component**: kept as a focus ring only. Its glow alone does not meet 3:1 on every surface, so the solid outline stays under it.
- **Markdown Graphs code**: not used, licence not verifiable. The heat map is original.
- **Sonner**: see above.
