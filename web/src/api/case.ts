// The cloud speaks snake_case JSON, the portal's types are camelCase. One conversion at the
// edge keeps both sides idiomatic.

const toCamel = (s: string) => s.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());
const toSnake = (s: string) => s.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);

function walk(value: unknown, rename: (k: string) => string): unknown {
	if (Array.isArray(value)) return value.map((v) => walk(v, rename));
	if (value && typeof value === "object" && Object.getPrototypeOf(value) === Object.prototype) {
		return Object.fromEntries(Object.entries(value).map(([k, v]) => [rename(k), walk(v, rename)]));
	}
	return value;
}

export const camelize = <T>(value: unknown): T => walk(value, toCamel) as T;
export const snakeize = (value: unknown): unknown => walk(value, toSnake);
