import LayoutDashboardIcon from '@lucide/svelte/icons/layout-dashboard';
import CircleDotIcon from '@lucide/svelte/icons/circle-dot';
import GitPullRequestIcon from '@lucide/svelte/icons/git-pull-request';
import TagsIcon from '@lucide/svelte/icons/tags';
import ScanSearchIcon from '@lucide/svelte/icons/scan-search';
import RadarIcon from '@lucide/svelte/icons/radar';
import WandSparklesIcon from '@lucide/svelte/icons/wand-sparkles';
import GitMergeIcon from '@lucide/svelte/icons/git-merge';
import LightbulbIcon from '@lucide/svelte/icons/lightbulb';
import ListOrderedIcon from '@lucide/svelte/icons/list-ordered';
import ZapIcon from '@lucide/svelte/icons/zap';
import GaugeIcon from '@lucide/svelte/icons/gauge';
import ChartLineIcon from '@lucide/svelte/icons/chart-line';
import ChartColumnIcon from '@lucide/svelte/icons/chart-column';
import NetworkIcon from '@lucide/svelte/icons/network';
import ChartPieIcon from '@lucide/svelte/icons/chart-pie';
import FileTextIcon from '@lucide/svelte/icons/file-text';
import ScrollTextIcon from '@lucide/svelte/icons/scroll-text';
import ActivityIcon from '@lucide/svelte/icons/activity';
import ClockIcon from '@lucide/svelte/icons/clock';
import TerminalIcon from '@lucide/svelte/icons/terminal';
import Undo2Icon from '@lucide/svelte/icons/undo-2';
import DatabaseBackupIcon from '@lucide/svelte/icons/database-backup';
import UsersIcon from '@lucide/svelte/icons/users';
import SettingsIcon from '@lucide/svelte/icons/settings';
import SearchIcon from '@lucide/svelte/icons/search';
import BotIcon from '@lucide/svelte/icons/bot';
import ChartAreaIcon from '@lucide/svelte/icons/chart-area';
import SquareActivityIcon from '@lucide/svelte/icons/square-activity';
import ShieldCheckIcon from '@lucide/svelte/icons/shield-check';
import WorkflowIcon from '@lucide/svelte/icons/workflow';

export type NavIcon = typeof LayoutDashboardIcon;

/** Sidebar groups. `Main` has no visible header; `Admin` starts folded. */
export type NavSection = 'Main' | 'Automation' | 'Insights' | 'Activity' | 'Admin';

export type NavItem = {
	href: string;
	label: string;
	icon: NavIcon;
	section: NavSection;
	/** Rendered as a sub-item under the item with this href. */
	parent?: string;
	/** Extra words the command palette matches on. */
	keywords?: string;
	/** Second key of a Linear-style "G then <key>" shortcut. */
	shortcut?: string;
	/** Status counter shown as a badge next to the item. */
	count?: 'open_issues' | 'open_prs' | 'untriaged' | 'conflicts';
	/** Highlight a non-zero count: it is work waiting on someone. */
	countAttention?: boolean;
	/** When set, hide the item unless `license.features[id].enabled === true`. */
	feature?: string;
	/** Hide the item unless the daemon reports `is_pro`. For Pro-only
	 *  pages that have no licence feature id of their own (the route
	 *  does not exist at all in an OSS build, so the link would 404). */
	proOnly?: boolean;
};

export const sectionOrder: NavSection[] = ['Main', 'Automation', 'Insights', 'Activity', 'Admin'];

/** One icon per group, used when the sidebar is collapsed to icons: each
 *  group then shows as a single button that opens its pages in a flyout. */
export const sectionIcons: Record<Exclude<NavSection, 'Main'>, NavIcon> = {
	Automation: BotIcon,
	Insights: ChartAreaIcon,
	Activity: SquareActivityIcon,
	Admin: ShieldCheckIcon
};

export const allNavItems: NavItem[] = [
	{ href: '/', label: 'Dashboard', icon: LayoutDashboardIcon, section: 'Main', keywords: 'home overview', shortcut: 'd' },
	{ href: '/issues', label: 'Issues', icon: CircleDotIcon, section: 'Main', shortcut: 'i', count: 'open_issues' },
	{ href: '/prs', label: 'Pull Requests', icon: GitPullRequestIcon, section: 'Main', keywords: 'pr', shortcut: 'p', count: 'open_prs' },

	{ href: '/triage', label: 'Triage', icon: TagsIcon, section: 'Automation', keywords: 'labels', shortcut: 't', count: 'untriaged', countAttention: true },
	{ href: '/review', label: 'Code Review', icon: ScanSearchIcon, section: 'Automation', shortcut: 'r' },
	{ href: '/review/radar', label: 'Radar', icon: RadarIcon, section: 'Automation', parent: '/review', keywords: 'review radar' },
	{ href: '/autofix', label: 'Auto-fix', icon: WandSparklesIcon, section: 'Automation', keywords: 'fix' },
	{ href: '/conflicts', label: 'Conflicts', icon: GitMergeIcon, section: 'Automation', count: 'conflicts', countAttention: true },
	{ href: '/improve', label: 'Improvements', icon: LightbulbIcon, section: 'Automation', keywords: 'improve ideas' },
	{ href: '/queue', label: 'Merge Queue', icon: ListOrderedIcon, section: 'Automation', shortcut: 'q' },
	{ href: '/actions', label: 'Actions', icon: ZapIcon, section: 'Automation' },
	{ href: '/pipelines', label: 'Pipelines', icon: WorkflowIcon, section: 'Automation', keywords: 'templates routes steps workflow' },

	{ href: '/summary', label: 'Summary', icon: GaugeIcon, section: 'Insights' },
	{ href: '/pr-insights', label: 'PR Insights', icon: ChartLineIcon, section: 'Insights' },
	{ href: '/issue-insights', label: 'Issue Insights', icon: ChartColumnIcon, section: 'Insights' },
	{ href: '/graphs', label: 'Graphs', icon: NetworkIcon, section: 'Insights' },
	{ href: '/usage', label: 'Usage', icon: ChartPieIcon, section: 'Insights', keywords: 'cost tokens' },
	{ href: '/reports', label: 'Reports', icon: FileTextIcon, section: 'Insights' },
	{ href: '/changelog', label: 'Changelog', icon: ScrollTextIcon, section: 'Insights', keywords: 'release notes' },

	{ href: '/activity', label: 'Activity', icon: ActivityIcon, section: 'Activity' },
	{ href: '/history', label: 'History', icon: ClockIcon, section: 'Activity' },
	{ href: '/logs', label: 'Logs', icon: TerminalIcon, section: 'Activity' },

	{ href: '/revert', label: 'Revert', icon: Undo2Icon, section: 'Admin', keywords: 'undo rollback' },
	{ href: '/backups', label: 'Backups', icon: DatabaseBackupIcon, section: 'Admin' },
	{ href: '/users', label: 'Users', icon: UsersIcon, section: 'Admin', proOnly: true, keywords: 'roles' }
];

/** Pinned above the user menu, outside the scrolling groups. */
export const settingsItem: NavItem = {
	href: '/settings',
	label: 'Settings',
	icon: SettingsIcon,
	section: 'Admin',
	keywords: 'repos secrets license ai provider',
	shortcut: 's'
};

export const searchItem: NavItem = {
	href: '/search',
	label: 'Search issues & PRs',
	icon: SearchIcon,
	section: 'Main'
};

/**
 * The nav item a path belongs to: the longest href that is the path itself
 * or a parent segment of it, so `/prs/42` highlights Pull Requests and
 * `/review/radar` highlights Radar rather than Code Review.
 */
export function activeHref(path: string, items: NavItem[]): string | null {
	let best: string | null = null;
	for (const i of items) {
		const match = i.href === '/' ? path === '/' : path === i.href || path.startsWith(i.href + '/');
		if (match && (best === null || i.href.length > best.length)) best = i.href;
	}
	return best;
}
