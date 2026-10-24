// Runs before the first paint so the saved theme and accent never flash. It is an external file
// because the CSP has no 'unsafe-inline' for scripts.
(() => {
	try {
		const root = document.documentElement;
		const theme = localStorage.getItem("anyplay.theme");
		if (theme === "dark" || theme === "light") root.setAttribute("data-theme", theme);
		const accent = localStorage.getItem("anyplay.accent");
		if (accent) root.setAttribute("data-accent", accent);
	} catch (_) {
		// storage blocked: the defaults apply
	}
})();
