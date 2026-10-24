import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

afterEach(() => {
	cleanup();
	vi.useRealTimers();
	try {
		localStorage.clear();
		sessionStorage.clear();
	} catch {
		// not available in this environment
	}
});

// jsdom has no layout engine; these are the browser APIs the UI libraries ask for.
class ResizeObserverStub {
	observe() {}
	unobserve() {}
	disconnect() {}
}
globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
window.matchMedia ??= ((query: string) => ({
	matches: false,
	media: query,
	onchange: null,
	addEventListener() {},
	removeEventListener() {},
	addListener() {},
	removeListener() {},
	dispatchEvent: () => false,
})) as unknown as typeof window.matchMedia;
Element.prototype.getAnimations ??= () => [];
Element.prototype.scrollIntoView ??= () => {};
window.scrollTo = (() => {}) as typeof window.scrollTo;
HTMLElement.prototype.hasPointerCapture ??= () => false;
HTMLElement.prototype.setPointerCapture ??= () => {};
HTMLElement.prototype.releasePointerCapture ??= () => {};
