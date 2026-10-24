// Runs before the first paint so the saved theme and accent never flash. It is an external file
// because the CSP has no 'unsafe-inline' for scripts.
(() => {
	try {
		var root = document.documentElement;
		var theme = localStorage.getItem("anyplay.theme");
		if (theme === "dark" || theme === "light") root.setAttribute("data-theme", theme);
		var accent = localStorage.getItem("anyplay.accent");
		if (accent) root.setAttribute("data-accent", accent);
	} catch (_) {}
})();
