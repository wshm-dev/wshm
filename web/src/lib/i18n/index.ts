import { writable, derived, get } from 'svelte/store';
import en from './en.json';
import fr from './fr.json';
import es from './es.json';
import de from './de.json';
import it from './it.json';
import pt from './pt.json';
import nl from './nl.json';
import pl from './pl.json';
import ru from './ru.json';
import zh from './zh.json';
import ja from './ja.json';
import ko from './ko.json';
import ar from './ar.json';

export type Locale =
	| 'en' | 'fr' | 'es' | 'de' | 'it' | 'pt' | 'nl' | 'pl' | 'ru'
	| 'zh' | 'ja' | 'ko' | 'ar';

export const LOCALES: { code: Locale; flag: string; label: string; dir?: 'rtl' }[] = [
	{ code: 'en', flag: '🇬🇧', label: 'English' },
	{ code: 'fr', flag: '🇫🇷', label: 'Français' },
	{ code: 'es', flag: '🇪🇸', label: 'Español' },
	{ code: 'de', flag: '🇩🇪', label: 'Deutsch' },
	{ code: 'it', flag: '🇮🇹', label: 'Italiano' },
	{ code: 'pt', flag: '🇵🇹', label: 'Português' },
	{ code: 'nl', flag: '🇳🇱', label: 'Nederlands' },
	{ code: 'pl', flag: '🇵🇱', label: 'Polski' },
	{ code: 'ru', flag: '🇷🇺', label: 'Русский' },
	{ code: 'zh', flag: '🇨🇳', label: '中文' },
	{ code: 'ja', flag: '🇯🇵', label: '日本語' },
	{ code: 'ko', flag: '🇰🇷', label: '한국어' },
	{ code: 'ar', flag: '🇸🇦', label: 'العربية', dir: 'rtl' }
];

const messages: Record<Locale, Record<string, string>> = {
	en, fr, es, de, it, pt, nl, pl, ru, zh, ja, ko, ar
};

const KNOWN: Locale[] = LOCALES.map((l) => l.code);

function detectInitial(): Locale {
	if (typeof window === 'undefined') return 'en';
	try {
		const saved = localStorage.getItem('wshm-locale') as Locale | null;
		if (saved && KNOWN.includes(saved)) return saved;
	} catch { /* ignore */ }
	// No language picker exists in the UI yet, and only a handful of routes
	// (Settings, Login, and — via the wshm-pro overlay — PR/Issue Insights)
	// are wired to `t()` while the rest of the app is hardcoded English —
	// so auto-switching those few pages off a browser/OS locale produced a
	// jarring mixed-language UI with no way to opt back out. Default to
	// English until a real language switcher lands; an explicit choice
	// (saved above) still always wins.
	return 'en';
}

export const locale = writable<Locale>(detectInitial());

locale.subscribe((v) => {
	if (typeof window === 'undefined') return;
	try { localStorage.setItem('wshm-locale', v); } catch { /* ignore */ }
	const dir = LOCALES.find((l) => l.code === v)?.dir ?? 'ltr';
	if (typeof document !== 'undefined') {
		document.documentElement.setAttribute('lang', v);
		document.documentElement.setAttribute('dir', dir);
	}
});

export function tr(key: string, l?: Locale): string {
	const lc = l ?? get(locale);
	return messages[lc][key] ?? messages.en[key] ?? key;
}

export const t = derived(locale, ($locale) => (key: string) => tr(key, $locale));
