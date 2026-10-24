// Every visible string lives in messages/en.json and comes out of the compiled Paraglide
// functions: `m.nav_overview()`. English is the only locale for now; a second one is a new file
// in messages/ plus its code in project.inlang/settings.json.
export * as m from "@/paraglide/messages.js";
export { getLocale } from "@/paraglide/runtime.js";
