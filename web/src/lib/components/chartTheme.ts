import { theme, palette } from '$lib/stores';

/** Chart ink colors resolved from the shadcn theme tokens on <html>.
 *  Canvas accepts the oklch() strings these vars hold in every browser
 *  that runs this dashboard (Chromium/WebKit/Gecko ≥ 2023). */
export function chartInk() {
	const s = getComputedStyle(document.documentElement);
	const v = (name: string, fallback: string) => s.getPropertyValue(name).trim() || fallback;
	return {
		tick: v('--muted-foreground', '#6b7280'),
		label: v('--muted-foreground', '#9ca3af'),
		grid: v('--border', '#374151'),
		surface: v('--card', '#1f2937')
	};
}

/** Increments on every theme switch; read it in a chart's $effect so the
 *  chart is rebuilt with the other mode's ink colors. */
export function onThemeChange(cb: () => void): () => void {
	// A color theme can re-tint surfaces too, so it also redraws charts.
	const skipFirst = () => {
		let first = true;
		return () => {
			if (first) {
				first = false;
				return;
			}
			cb();
		};
	};
	const offTheme = theme.subscribe(skipFirst());
	const offPalette = palette.subscribe(skipFirst());
	return () => {
		offTheme();
		offPalette();
	};
}
