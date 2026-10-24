// Generates the colour tokens and the contrast table.
//
//   node scripts/gen-tokens.mjs          writes src/styles/tokens.generated.css and docs/colour.md
//   node scripts/gen-tokens.mjs --check  exits 1 when either file is out of date (used in CI)
//
// Method (notes/research_notes/Startup UI Designsystem für AnyPlay/color_type_system.md, 3.1):
// ramps of 12 steps with the Radix roles, lightness and chroma taken from the median of the 25
// Radix scales, constant hue per ramp, chroma capped to a share of what sRGB can show at that
// lightness and hue. The neutral is the Radix gray lightness curve with chroma 0, because the
// launcher's chrome is pure white and black with alpha. Each accent keeps its launcher hue, and
// in dark mode step 9 is the launcher's own colour. Light mode is its own palette: step 9 is
// searched for the lightest value that still reaches the contrast the role needs.
//
// Everything is measured on the final hex values (WCAG 2.2 ratio and APCA Lc). APCA is the
// published SAPC 0.0.98G-4g formula, implemented below so the script has no licence to carry.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { formatHex, inGamut, oklch, rgb, wcagContrast } from "culori";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CSS_OUT = resolve(root, "src/styles/tokens.generated.css");
const DOC_OUT = resolve(root, "docs/colour.md");

// ---------------------------------------------------------------------------------------------
// Inputs

// Median OKLCH lightness and chroma per step over the 25 Radix scales (Radix Colors 3.0.0).
const MEDIAN = {
	light: {
		L: [0.993, 0.982, 0.959, 0.931, 0.9, 0.86, 0.807, 0.736],
		C: [0.003, 0.009, 0.022, 0.039, 0.053, 0.068, 0.087, 0.113, 0.179, 0.177, 0.129, 0.057],
	},
	dark: {
		L: [0.188, 0.211, 0.264, 0.305, 0.348, 0.396, 0.456, 0.532],
		C: [0.014, 0.019, 0.047, 0.07, 0.078, 0.087, 0.095, 0.115, 0.179, 0.172, 0.131, 0.053],
	},
};
// Radix gray lightness, steps 1 to 12.
const GRAY_L = {
	light: [0.991, 0.982, 0.955, 0.931, 0.907, 0.885, 0.851, 0.792, 0.643, 0.61, 0.49, 0.244],
	dark: [0.178, 0.213, 0.252, 0.285, 0.313, 0.348, 0.402, 0.489, 0.538, 0.583, 0.77, 0.949],
};

// The launcher's accents (docs/DESIGN.md 1.2). `white` is a neutral accent.
export const ACCENTS = {
	blue: { name: "Blue", hex: "#4F94FF" },
	mint: { name: "Mint", hex: "#5CEBBD" },
	coral: { name: "Coral", hex: "#FF8066" },
	violet: { name: "Violet", hex: "#A885FF" },
	gold: { name: "Gold", hex: "#FFCC5C" },
	white: { name: "White", hex: "#F0F0F0", neutral: true },
};
export const DEFAULT_ACCENT = "blue";

// The launcher's trophy grades (docs/DESIGN.md 1.2), the dark-mode anchors.
const GRADES = {
	platinum: { name: "Platinum", hex: "#BDD6F7" },
	gold: { name: "Gold", hex: "#F7C757" },
	silver: { name: "Silver", hex: "#CCD1DB" },
	bronze: { name: "Bronze", hex: "#D48C5C" },
};

// Status hues. Step 9 sits at one common lightness, so danger, warning and success read as
// equally loud.
const STATUS = {
	danger: { hue: 25 },
	warning: { hue: 78 },
	success: { hue: 152 },
};
const STATUS_L9 = { dark: 0.76, light: 0.56 };

// ---------------------------------------------------------------------------------------------
// Colour helpers

const maxC = (l, h) => {
	let lo = 0;
	let hi = 0.45;
	for (let i = 0; i < 28; i++) {
		const m = (lo + hi) / 2;
		if (inGamut("rgb")({ mode: "oklch", l, c: m, h })) lo = m;
		else hi = m;
	}
	return lo;
};
const hexOf = (l, c, h) => formatHex({ mode: "oklch", l, c: Math.min(c, maxC(l, h)), h });
const mk = (l, c, h) => {
	const cc = Math.min(c, maxC(l, h));
	return {
		l,
		c: cc,
		h,
		hex: formatHex({ mode: "oklch", l, c: cc, h }),
		ok: `oklch(${(l * 100).toFixed(1)}% ${cc.toFixed(3)} ${Math.round(h)})`,
	};
};
const wcag = (a, b) => wcagContrast(a, b);

// APCA 0.0.98G-4g (SAPC), polarity kept: positive = dark text on light, negative = light on dark.
function luminance(hex) {
	const c = rgb(hex);
	const lin = (v) => v ** 2.4;
	return 0.2126729 * lin(c.r) + 0.7151522 * lin(c.g) + 0.072175 * lin(c.b);
}
export function apca(text, bg) {
	const clamp = (y) => (y >= 0.022 ? y : y + (0.022 - y) ** 1.414);
	const yt = clamp(luminance(text));
	const yb = clamp(luminance(bg));
	if (Math.abs(yb - yt) < 0.0005) return 0;
	if (yb > yt) {
		const s = (yb ** 0.56 - yt ** 0.57) * 1.14;
		return (s < 0.1 ? 0 : s - 0.027) * 100;
	}
	const s = (yb ** 0.65 - yt ** 0.62) * 1.14;
	return (s > -0.1 ? 0 : s + 0.027) * 100;
}
const lc = (t, b) => Math.abs(apca(t, b));

/** Composite a colour with alpha over an opaque background (hex in, hex out). */
function over(fg, alpha, bg) {
	const f = rgb(fg);
	const b = rgb(bg);
	return formatHex({
		mode: "rgb",
		r: f.r * alpha + b.r * (1 - alpha),
		g: f.g * alpha + b.g * (1 - alpha),
		b: f.b * alpha + b.b * (1 - alpha),
	});
}

// ---------------------------------------------------------------------------------------------
// Ramps

function grayRamp(mode) {
	return GRAY_L[mode].map((l) => mk(l, 0, 0));
}

/** Step lightness for the shared steps of a coloured ramp. */
function shared(mode, i) {
	if (i < 8) return MEDIAN[mode].L[i];
	return null;
}

/**
 * A coloured ramp. `anchor` is the lightness and chroma of step 9 in dark mode; light mode
 * searches its own step 9 with `need` (a function: hex -> bool).
 */
function ramp(hue, mode, { anchor, need, neutral = false, text = {} }) {
	const out = [];
	const Rt = mode === "light" ? 0.68 : 0.8;
	const cap = (l, c, ratio) => Math.min(c, ratio * maxC(l, hue));
	const base = (i) => {
		const l = shared(mode, i);
		return mk(l, neutral ? 0 : cap(l, MEDIAN[mode].C[i], Rt), hue);
	};
	for (let i = 0; i < 8; i++) out.push(base(i));

	let l9;
	let c9;
	if (mode === "dark") {
		l9 = anchor.l;
		c9 = anchor.c;
	} else {
		// lightest step 9 that still passes the role, same hue, same share of the gamut
		const ratio = Math.min(0.92, anchor.c / maxC(anchor.l, hue));
		l9 = Math.min(anchor.l, 0.74);
		const test = (l) => need(hexOf(l, neutral ? 0 : ratio * maxC(l, hue), hue));
		while (!test(l9) && l9 > 0.2) l9 -= 0.005;
		c9 = neutral ? 0 : ratio * maxC(l9, hue);
	}
	out.push(mk(l9, c9, hue));
	const l10 = mode === "dark" ? Math.min(l9 + 0.03, 0.97) : l9 - 0.025;
	out.push(mk(l10, c9 * (mode === "dark" ? 1 : 0.98), hue));

	// 11 is the text step, 12 the high-contrast text.
	const l11 = text.l11 ?? (mode === "dark" ? 0.845 : 0.51);
	const l12 = text.l12 ?? (mode === "dark" ? 0.93 : 0.32);
	out.push(mk(l11, neutral ? 0 : cap(l11, MEDIAN[mode].C[10], Rt), hue));
	out.push(mk(l12, neutral ? 0 : cap(l12, MEDIAN[mode].C[11], Rt), hue));
	return out;
}

const INK = { dark: grayRamp("dark")[0].hex, light: "#ffffff" };

function accentRamp(key, mode, grayR) {
	const def = ACCENTS[key];
	const a = oklch(def.hex);
	const bg = grayR[0].hex;
	const raised = grayR[2].hex;
	const hue = def.neutral ? 0 : a.h;
	// light mode: step 9 is a graphic (progress, ring, switch track) and must reach 3.1:1 on
	// the page, the surface and the raised surface
	const need = (hex) =>
		wcag(hex, bg) >= 3.1 &&
		wcag(hex, grayR[1].hex) >= 3.1 &&
		wcag(hex, raised) >= 3.1 &&
		wcag(hex, grayR[3].hex) >= 3.1;
	const r = ramp(hue, mode, {
		anchor: { l: a.l, c: def.neutral ? 0 : a.c },
		need,
		neutral: !!def.neutral,
		text: def.neutral ? (mode === "dark" ? { l11: 0.86, l12: 0.96 } : { l11: 0.45, l12: 0.2 }) : {},
	});
	if (def.neutral && mode === "light") {
		r[8] = mk(0.32, 0, 0);
		r[9] = mk(0.24, 0, 0);
	}
	if (def.neutral && mode === "dark") {
		r[8] = mk(a.l, 0, 0);
		r[9] = mk(0.975, 0, 0);
	}
	return r;
}

/** Pick the label colour for a solid fill: whichever of the two inks reads better. */
function onFill(fillHex, mode) {
	const dark = INK.dark;
	const light = "#ffffff";
	return wcag(dark, fillHex) >= wcag(light, fillHex) ? dark : light;
}

function statusRamp(key, mode, grayR) {
	const hue = STATUS[key].hue;
	const l9 = STATUS_L9[mode];
	const c9 = 0.85 * maxC(l9, hue);
	return ramp(hue, mode, {
		anchor: { l: l9, c: c9 },
		need: (hex) => wcag(onFill(hex, mode), hex) >= 4.6,
	});
}

function gradeRamp(key, mode, grayR) {
	const a = oklch(GRADES[key].hex);
	const bg = grayR[0].hex;
	const silver = key === "silver";
	return ramp(a.h, mode, {
		anchor: { l: a.l, c: a.c },
		need: (hex) => wcag(hex, bg) >= 3.1 && wcag(hex, grayR[2].hex) >= 3.1,
		text: silver ? { l11: mode === "dark" ? 0.85 : 0.5, l12: mode === "dark" ? 0.93 : 0.32 } : {},
	});
}

// ---------------------------------------------------------------------------------------------
// Build the palette

const modes = ["dark", "light"];
const gray = { dark: grayRamp("dark"), light: grayRamp("light") };

const accent = {};
for (const k of Object.keys(ACCENTS)) {
	accent[k] = {};
	for (const m of modes) accent[k][m] = accentRamp(k, m, gray[m]);
}
const status = {};
for (const k of Object.keys(STATUS)) {
	status[k] = {};
	for (const m of modes) status[k][m] = statusRamp(k, m, gray[m]);
}
const grade = {};
for (const k of Object.keys(GRADES)) {
	grade[k] = {};
	for (const m of modes) grade[k][m] = gradeRamp(k, m, gray[m]);
}

// Picks that depend on measurement.
const picks = {};
for (const m of modes) {
	const g = gray[m].map((r) => r.hex);
	// control border: the first gray from step 7 up that reaches 3:1 on the page and on the
	// surface and the raised surface it can sit on
	let control = 9;
	for (let i = 6; i < 11; i++) {
		if (wcag(g[i], g[0]) >= 3 && wcag(g[i], g[1]) >= 3 && wcag(g[i], g[2]) >= 3) {
			control = i + 1;
			break;
		}
	}
	picks[m] = {
		control,
		// ink: the launcher's focused surface (solid near-white, text near-black) in dark mode,
		// its mirror in light mode
		ink: m === "dark" ? "#f4f4f4" : g[11],
		inkHover: m === "dark" ? "#ffffff" : "#000000",
		onInk: m === "dark" ? g[0] : "#ffffff",
	};
}

// ---------------------------------------------------------------------------------------------
// CSS

const OK = "@supports (color: oklch(0 0 0))";

function ld(light, dark, key) {
	return `light-dark(${light[key]}, ${dark[key]})`;
}

/** Declarations for one ramp: hex first, then the same colours in OKLCH where supported. */
function rampDecls(prefix, light, dark, steps = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]) {
	const hexLines = [];
	const okLines = [];
	for (const s of steps) {
		hexLines.push(`  --${prefix}-${s}: ${ld(light[s - 1], dark[s - 1], "hex")};`);
		okLines.push(`  --${prefix}-${s}: ${ld(light[s - 1], dark[s - 1], "ok")};`);
	}
	return { hexLines, okLines };
}

function buildCss() {
	const hex = [];
	const ok = [];
	const push = (d) => {
		hex.push(...d.hexLines);
		ok.push(...d.okLines);
	};
	push(rampDecls("gray", gray.light, gray.dark));
	for (const k of Object.keys(STATUS)) push(rampDecls(k, status[k].light, status[k].dark));
	for (const k of Object.keys(GRADES))
		push(rampDecls(`grade-${k}`, grade[k].light, grade[k].dark, [3, 5, 7, 9, 11]));

	// The launcher's own accent colours, fixed in both modes: another player's avatar and
	// banner use their accent whatever accent this browser has picked.
	const swatches = Object.keys(ACCENTS).map(
		(k) => `  --swatch-${k}: ${ACCENTS[k].hex.toLowerCase()};`,
	);

	const pickLines = (key, vals) => `  --${key}: light-dark(${vals.light}, ${vals.dark});`;
	const picked = [
		pickLines("control-border", {
			light: gray.light[picks.light.control - 1].hex,
			dark: gray.dark[picks.dark.control - 1].hex,
		}),
		pickLines("ink", { light: picks.light.ink, dark: picks.dark.ink }),
		pickLines("ink-hover", { light: picks.light.inkHover, dark: picks.dark.inkHover }),
		pickLines("on-ink", { light: picks.light.onInk, dark: picks.dark.onInk }),
		pickLines("on-danger", {
			light: onFill(status.danger.light[8].hex, "light"),
			dark: onFill(status.danger.dark[8].hex, "dark"),
		}),
	];

	const accentBlock = (k, selector) => {
		const d = rampDecls("accent", accent[k].light, accent[k].dark);
		const onL = onFill(accent[k].light[8].hex, "light");
		const onD = onFill(accent[k].dark[8].hex, "dark");
		const extra = [`  --on-accent: light-dark(${onL}, ${onD});`];
		return [
			`${selector} {`,
			...d.hexLines,
			...extra,
			"}",
			`${OK} {`,
			`${selector} {`,
			...d.okLines,
			"}",
			"}",
		].join("\n");
	};

	const parts = [];
	parts.push(
		"/* Generated by scripts/gen-tokens.mjs. Do not edit; change the script and run `pnpm tokens`.",
		"   Every colour is written as an sRGB hex first and again as OKLCH where the browser has it.",
		"   light-dark() picks the mode from color-scheme, which src/styles/tokens.css sets. */",
		":root {",
		...hex,
		...picked,
		...swatches,
		"}",
		`${OK} {`,
		":root {",
		...ok,
		"}",
		"}",
		"",
		accentBlock(DEFAULT_ACCENT, ":root"),
	);
	for (const k of Object.keys(ACCENTS)) {
		if (k === DEFAULT_ACCENT) continue;
		parts.push("", accentBlock(k, `:root[data-accent="${k}"]`));
	}
	return `${parts.join("\n")}\n`;
}

// ---------------------------------------------------------------------------------------------
// Measurements

const f2 = (n) => n.toFixed(2);
const rows = [];
const failures = [];

/**
 * gate kinds: body (4.5 and Lc 75), label (4.5 and Lc 60), ui (3 and Lc 30), large (3 and Lc 45)
 */
function gateOf(kind, w, l) {
	if (kind === "body")
		return {
			pass: w >= 4.5,
			text: `${w >= 4.5 ? "AA" : "fails AA"}${w >= 7 ? "/AAA" : ""}, ${l >= 75 ? "Lc 75" : l >= 60 ? "Lc 60 (labels)" : "below Lc 60"}`,
		};
	if (kind === "label")
		return {
			pass: w >= 4.5,
			text: `${w >= 4.5 ? "AA" : "fails AA"}, ${l >= 60 ? "Lc 60" : "below Lc 60"}`,
		};
	if (kind === "ui")
		return {
			pass: w >= 3,
			text: `${w >= 3 ? "3:1" : "fails 3:1"}, ${l >= 30 ? "Lc 30" : "below Lc 30"}`,
		};
	return { pass: true, text: "info" };
}
function measure(group, name, fg, bg, kind) {
	const w = wcag(fg, bg);
	const l = lc(fg, bg);
	const g = gateOf(kind, w, l);
	rows.push({ group, name, fg, bg, w, l, gate: g.text, pass: g.pass, kind });
	if (!g.pass) failures.push(`${group}: ${name} ${f2(w)}:1`);
	return { w, l };
}

function runMeasures() {
	for (const m of modes) {
		const g = gray[m].map((r) => r.hex);
		const a = accent[DEFAULT_ACCENT][m].map((r) => r.hex);
		const G = `${m}, text`;
		measure(G, "fg (gray 12) on page (gray 1)", g[11], g[0], "body");
		measure(G, "fg on surface (gray 2)", g[11], g[1], "body");
		measure(G, "fg on raised (gray 3)", g[11], g[2], "body");
		measure(G, "fg on hover (gray 4)", g[11], g[3], "body");
		measure(G, "fg-muted (gray 11) on page", g[10], g[0], "body");
		measure(G, "fg-muted on surface", g[10], g[1], "body");
		measure(G, "fg-muted on raised", g[10], g[2], "body");
		measure(G, "fg-muted on hover (gray 4)", g[10], g[3], "body");
		measure(G, "fg-muted on active (gray 5)", g[10], g[4], "body");
		measure(G, "fg-faint (gray 10) on page, disabled and decoration only", g[9], g[0], "info");
		measure(G, "ink button: on-ink on ink", picks[m].onInk, picks[m].ink, "body");
		measure(G, "ink button hover: on-ink on ink-hover", picks[m].onInk, picks[m].inkHover, "body");

		const L = `${m}, borders and rings`;
		const lineBg = over(m === "dark" ? "#ffffff" : "#000000", 0.08, g[0]);
		const lineStrong = over(m === "dark" ? "#ffffff" : "#000000", 0.14, g[0]);
		measure(L, "hairline (8% alpha) over page", lineBg, g[0], "info");
		measure(L, "hairline strong (14% alpha) over page", lineStrong, g[0], "info");
		const cb = gray[m][picks[m].control - 1].hex;
		measure(L, `control border (gray ${picks[m].control}) on page`, cb, g[0], "ui");
		measure(L, `control border on surface`, cb, g[1], "ui");
		measure(L, `control border on raised`, cb, g[2], "ui");
		measure(L, "surface (gray 2) edge against page, no border", g[1], g[0], "info");

		const A = `${m}, ${ACCENTS[DEFAULT_ACCENT].name} accent`;
		measure(A, "focus ring / progress fill (accent 9) on page", a[8], g[0], "ui");
		measure(A, "accent 9 on surface", a[8], g[1], "ui");
		measure(A, "accent 9 on raised", a[8], g[2], "ui");
		measure(A, "accent 9 on progress track (gray 4)", a[8], g[3], "ui");
		measure(A, "accent text (accent 11) on page", a[10], g[0], "body");
		measure(A, "accent text on surface", a[10], g[1], "body");
		measure(A, "accent text on accent-3 (selected row)", a[10], a[2], "body");
		measure(A, "accent 12 on accent-3", a[11], a[2], "body");
		measure(A, "on-accent (switch knob, check) on accent 9", onFill(a[8], m), a[8], "ui");

		for (const k of Object.keys(STATUS)) {
			const s = status[k][m].map((r) => r.hex);
			const S = `${m}, ${k}`;
			measure(S, `${k} text (11) on page`, s[10], g[0], "body");
			measure(S, `${k} text (11) on surface`, s[10], g[1], "body");
			measure(S, `${k} text (11) on ${k}-3 (callout)`, s[10], s[2], "body");
			measure(S, `label on ${k} solid (9)`, onFill(s[8], m), s[8], "label");
			measure(S, `${k} icon (9) on page`, s[8], g[0], "ui");
		}
		for (const k of Object.keys(GRADES)) {
			const s = grade[k][m].map((r) => r.hex);
			const S = `${m}, trophy ${k}`;
			measure(S, `${k} fill (9) on page`, s[8], g[0], "ui");
			measure(S, `${k} fill (9) on raised`, s[8], g[2], "ui");
			measure(S, `${k} text (11) on ${k}-3 (chip)`, s[10], s[2], "body");
			measure(S, `${k} text (11) on page`, s[10], g[0], "body");
		}
	}
}
runMeasures();

/** One line per accent and mode, the pairs that decide whether the accent is safe. */
const accentMatrix = [];
for (const k of Object.keys(ACCENTS)) {
	for (const m of modes) {
		const g = gray[m].map((r) => r.hex);
		const a = accent[k][m].map((r) => r.hex);
		const onA = onFill(a[8], m);
		const r9 = wcag(a[8], g[0]);
		const r9raised = wcag(a[8], g[2]);
		const t11 = wcag(a[10], g[0]);
		const t11sel = wcag(a[10], a[2]);
		const knob = wcag(onA, a[8]);
		const checks = [
			["step 9 on page", r9, 3],
			["step 9 on raised", r9raised, 3],
			["text on page", t11, 4.5],
			["text on selected", t11sel, 4.5],
			["knob on step 9", knob, 3],
		];
		for (const [n, v, need] of checks) {
			if (v < need) failures.push(`${ACCENTS[k].name} ${m}: ${n} ${f2(v)}:1 (needs ${need})`);
		}
		accentMatrix.push({
			k,
			m,
			a9: a[8],
			a11: a[10],
			r9,
			l9: lc(a[8], g[0]),
			t11,
			t11sel,
			knob,
			a3: a[2],
		});
	}
}

// ---------------------------------------------------------------------------------------------
// Markdown

function buildDoc() {
	const L = [];
	L.push(
		"# Colour",
		"",
		"Generated by `scripts/gen-tokens.mjs` (`pnpm tokens`). Do not edit by hand; change the script and run it again. CI runs it with `--check`.",
		"",
		"## Method",
		"",
		"- **Ramps, not colours.** Neutral, six accents, three status colours and four trophy grades are 12-step ramps with the Radix roles: 1 page, 2 surface, 3 raised, 4 hover, 5 active, 6 subtle border, 7 border, 8 strong border, 9 solid, 10 hover solid, 11 text, 12 high-contrast text.",
		"- **Shape of a ramp.** Lightness and chroma of steps 1 to 8, 11 and 12 are the per-step medians over the 25 Radix scales. The hue is constant along the ramp. Chroma is capped at 80% (dark) or 68% (light) of what sRGB can show at that lightness and hue, so no hue is louder than another.",
		"- **Neutral.** The Radix gray lightness curve with chroma 0. The launcher's chrome is pure white and black with alpha, so the portal's grays carry no tint.",
		"- **Accents.** The six launcher accents keep their hue. In dark mode step 9 is the launcher colour itself. Light mode is a palette of its own, not an inversion: step 9 is the lightest value of the same hue and chroma share that reaches 3.1:1 on the page, the surface and the raised surface, because it is used as progress fill, focus ring and switch track.",
		"- **Ink, not accent.** The primary button is the launcher's focused surface: near-white with near-black text in dark mode, the mirror in light mode. Accent colour marks progress, the selected state, focus and the online dot.",
		"- **Status.** One common step-9 lightness (0.76 dark, 0.56 light), so danger, warning and success are equally loud. The label on a solid is whichever of near-black and white reads better.",
		"- **Trophy grades.** Dark mode anchors are the launcher's Platinum `#BDD6F7`, Gold `#F7C757`, Silver `#CCD1DB` and Bronze `#D48C5C`. Light mode steps 9 are darkened until they reach 3.1:1.",
		"- **Fallbacks.** Every token is written as an sRGB hex and again as OKLCH inside `@supports (color: oklch(0 0 0))`. The mode is picked with `light-dark()` from `color-scheme`. All values are inside sRGB, so the two forms render the same.",
		"- **Measured, not estimated.** Ratios are WCAG 2.2 on the final hex values. Lc is APCA (SAPC 0.0.98G-4g). Alpha hairlines are composited over the page before they are measured. Gates: body text 4.5:1 and Lc 75, labels 4.5:1 and Lc 60, UI components and focus indicators 3:1 and Lc 30.",
		"",
		"## Neutral",
		"",
		"| step | role | dark | light |",
		"|---|---|---|---|",
	);
	const roles = [
		"page",
		"surface",
		"raised",
		"hover",
		"active",
		"subtle border",
		"border",
		"strong border",
		"solid",
		"hover solid",
		"muted text",
		"text",
	];
	for (let i = 0; i < 12; i++) {
		L.push(
			`| ${i + 1} | ${roles[i]} | \`${gray.dark[i].hex}\` ${gray.dark[i].ok} | \`${gray.light[i].hex}\` ${gray.light[i].ok} |`,
		);
	}
	L.push(
		"",
		`Picked by measurement: control border is gray ${picks.dark.control} in dark mode and gray ${picks.light.control} in light mode (the first step from 7 up that reaches 3:1 on page, surface and raised surface). Ink button: \`${picks.dark.ink}\` on dark, \`${picks.light.ink}\` on light.`,
		"",
		"## Accents (step 9, 10, 11)",
		"",
		"| accent | dark 9 | dark 10 | dark 11 | light 9 | light 10 | light 11 |",
		"|---|---|---|---|---|---|---|",
	);
	for (const k of Object.keys(ACCENTS)) {
		const d = accent[k].dark;
		const l = accent[k].light;
		L.push(
			`| ${ACCENTS[k].name}${k === DEFAULT_ACCENT ? " (default)" : ""} | \`${d[8].hex}\` | \`${d[9].hex}\` | \`${d[10].hex}\` | \`${l[8].hex}\` | \`${l[9].hex}\` | \`${l[10].hex}\` |`,
		);
	}
	L.push(
		"",
		"Dark step 9 equals the launcher colour: " +
			Object.keys(ACCENTS)
				.map((k) => `${ACCENTS[k].name} \`${ACCENTS[k].hex}\``)
				.join(", ") +
			".",
		"",
	);
	L.push(
		"## Status and trophy grades (steps 3, 9, 11)",
		"",
		"| colour | dark 3 | dark 9 | dark 11 | light 3 | light 9 | light 11 |",
		"|---|---|---|---|---|---|---|",
	);
	for (const k of Object.keys(STATUS)) {
		const d = status[k].dark;
		const l = status[k].light;
		L.push(
			`| ${k} | \`${d[2].hex}\` | \`${d[8].hex}\` | \`${d[10].hex}\` | \`${l[2].hex}\` | \`${l[8].hex}\` | \`${l[10].hex}\` |`,
		);
	}
	for (const k of Object.keys(GRADES)) {
		const d = grade[k].dark;
		const l = grade[k].light;
		L.push(
			`| ${GRADES[k].name} | \`${d[2].hex}\` | \`${d[8].hex}\` | \`${d[10].hex}\` | \`${l[2].hex}\` | \`${l[8].hex}\` | \`${l[10].hex}\` |`,
		);
	}
	L.push(
		"",
		"## Every accent, both modes",
		"",
		"The pairs that decide whether an accent is safe. Step 9 is progress fill, focus ring and switch track; step 11 is accent text; the knob is the check mark or switch knob on step 9.",
		"",
		"| accent | mode | step 9 | 9 on page | Lc | text on page | text on selected | knob on 9 |",
		"|---|---|---|---|---|---|---|---|",
	);
	for (const r of accentMatrix) {
		L.push(
			`| ${ACCENTS[r.k].name} | ${r.m} | \`${r.a9}\` | ${f2(r.r9)} | ${r.l9.toFixed(0)} | ${f2(r.t11)} | ${f2(r.t11sel)} | ${f2(r.knob)} |`,
		);
	}
	const groups = [...new Set(rows.map((r) => r.group))];
	L.push("", "## Pairs", "");
	for (const g of groups) {
		L.push(
			`### ${g}`,
			"",
			"| pair | fg | bg | WCAG 2.2 | Lc | gate |",
			"|---|---|---|---|---|---|",
		);
		for (const r of rows.filter((x) => x.group === g)) {
			L.push(
				`| ${r.name} | \`${r.fg}\` | \`${r.bg}\` | ${f2(r.w)} | ${r.l.toFixed(0)} | ${r.pass ? r.gate : `**${r.gate}**`} |`,
			);
		}
		L.push("");
	}
	L.push("## Gaps", "");
	if (failures.length === 0) L.push("Every pair above reaches its gate.");
	else {
		L.push(
			"Pairs that miss their gate. None of them carries required text or a required indicator unless the line says so:",
			"",
			...failures.map((x) => `- ${x}`),
		);
	}
	L.push(
		"",
		"Known and intended:",
		"",
		"- Hairlines (8% and 14% alpha) separate cards and rows. They are decoration; the control that needs a visible edge uses the control border above.",
		"- `fg-faint` (gray 10) is for disabled controls and decoration. Placeholders and helper text use `fg-muted`.",
		"- Gold and Mint in light mode are darker than the launcher colours because step 9 has to reach 3.1:1 on white.",
		"",
	);
	return `${L.join("\n")}\n`;
}

const css = buildCss();
const doc = buildDoc();

if (process.argv.includes("--check")) {
	const read = (p) => {
		try {
			return readFileSync(p, "utf8");
		} catch {
			return "";
		}
	};
	const same = read(CSS_OUT) === css && read(DOC_OUT) === doc;
	if (!same) {
		console.error("tokens are out of date: run `pnpm tokens` and commit the result");
		process.exit(1);
	}
	console.log("tokens are up to date");
} else {
	mkdirSync(dirname(CSS_OUT), { recursive: true });
	mkdirSync(dirname(DOC_OUT), { recursive: true });
	writeFileSync(CSS_OUT, css);
	writeFileSync(DOC_OUT, doc);
	console.log(`wrote ${CSS_OUT}\nwrote ${DOC_OUT}`);
	console.log(`${rows.length} pairs measured, ${failures.length} below their gate`);
	for (const f of failures) console.log(`  - ${f}`);
}
