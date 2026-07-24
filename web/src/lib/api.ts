import { get } from 'svelte/store';
import { selectedRepo } from './stores';

const BASE = '/api/v1';

function repoParams(): Record<string, string> {
	const repo = get(selectedRepo);
	return repo ? { repo } : {};
}

async function apiGet<T>(path: string, params?: Record<string, string | number>): Promise<T> {
	// `new URL` already splits "/logs?tail=100" into pathname="/logs" and
	// searchParams="tail=100". We prepend BASE to `url.pathname` (NOT the
	// raw `path`) so a literal '?' in the original path is not
	// percent-encoded into the pathname (oauth2-proxy then sees a path
	// that doesn't match any backend route and falls through to the SPA).
	const url = new URL(path, window.location.origin);
	url.pathname = `${BASE}${url.pathname}`;
	const merged = { ...repoParams(), ...params };
	for (const [key, value] of Object.entries(merged)) {
		url.searchParams.set(key, String(value));
	}
	const res = await fetch(url.toString());
	if (!res.ok) {
		throw new Error(`API error: ${res.status} ${res.statusText}`);
	}
	// oauth2-proxy returns HTML (200 or 302→follow→HTML) when the SSO
	// session expired silently. Without this guard `res.json()` throws
	// "Unexpected token '<'". Surface a clean message instead of
	// auto-redirecting (auto-redirect caused a re-login loop on the
	// /logs polling page).
	const ct = res.headers.get('content-type') ?? '';
	if (ct.includes('text/html')) {
		throw new Error('SSO session expired — refresh the page to sign in');
	}
	return res.json();
}

export interface RepoInfo {
	slug: string;
	open_issues: number;
	untriaged: number;
	open_prs: number;
	unanalyzed: number;
	conflicts: number;
	last_sync: string | null;
	apply: boolean;
}

export interface Status {
	open_issues: number;
	untriaged: number;
	open_prs: number;
	unanalyzed: number;
	conflicts: number;
	last_sync: string | null;
	repos: RepoInfo[];
}

export interface Issue {
	repo: string;
	/** Public web URL on the source forge (GitHub/GitLab/Gitea/Forgejo/Azure DevOps).
	 *  Optional because older daemon versions didn't include it. */
	url?: string | null;
	number: number;
	title: string;
	body: string | null;
	state: string;
	labels: string[];
	author: string | null;
	priority: string | null;
	category: string | null;
	pr_status: string | null;
	created_at: string;
	updated_at: string;
	/** GitHub 👍 (+1) reaction count. Sizes the issue node in the label graph. */
	reactions_plus1?: number | null;
	/** "Grand domains" the AI triage tagged this issue with. */
	domains?: string[] | null;
}

export interface PullRequest {
	repo: string;
	/** Public web URL on the source forge. Optional for backward compat. */
	url?: string | null;
	number: number;
	title: string;
	body: string | null;
	state: string;
	labels: string[];
	author: string | null;
	head_ref: string | null;
	base_ref: string | null;
	risk: string | null;
	risk_level: string | null;
	pr_type: string | null;
	summary: string | null;
	ci_status: string | null;
	mergeable: boolean | null;
	created_at: string;
	updated_at: string;
	/** GitHub review decision: "approved" | "changes_requested" | "review_required" | null. */
	review_decision?: string | null;
	/** When the current review decision was first observed (RFC3339). */
	review_decision_at?: string | null;
	/** GitHub 👍 (+1) reaction count on the PR. Optional — filled by the
	 *  pulls-reactions sync pass; sizes the PR node in the label graph. */
	reactions_plus1?: number | null;
	/** "Grand domains" the AI review tagged this PR with (codex, bun, …).
	 *  Optional — present once the PR has been analyzed. Drives the domain
	 *  filters in the PR network graph. */
	domains?: string[] | null;
}

export interface TriageResult {
	repo: string;
	issue_number: number;
	category: string;
	confidence: number;
	priority: string;
	summary: string | null;
	acted_at: string | null;
}

export interface QueueEntry {
	repo: string;
	number: number;
	title: string;
	score: number;
	ci_status: string | null;
	mergeable: boolean | null;
	risk_level: string | null;
	pr_type: string | null;
	created_at: string;
}

export interface ActivityEntry {
	id: number;
	action: string;
	target_type: string;
	target_number: number;
	summary: string;
	created_at: string;
}

export function fetchStatus(): Promise<Status> {
	return apiGet<Status>('/status');
}

export interface IssueBrief {
	number: number;
	title: string;
	priority: string | null;
	category: string | null;
	labels: string[];
	age_days: number;
}

export interface PrBrief {
	number: number;
	title: string;
	risk_level: string | null;
	ci_status: string | null;
	has_conflicts: boolean;
	age_days: number;
}

export interface Summary {
	repo: string;
	timestamp: string;
	open_issues: number;
	untriaged_issues: number;
	high_priority_issues: IssueBrief[];
	top_issues: IssueBrief[];
	open_prs: number;
	unanalyzed_prs: number;
	high_risk_prs: PrBrief[];
	top_prs: PrBrief[];
	conflicts: number;
}

export function fetchSummary(): Promise<Summary> {
	return apiGet<Summary>('/summary');
}

/** Standard envelope for paginated list endpoints. */
export interface Page<T> {
	items: T[];
	total: number;
	limit: number;
	offset: number;
}

export interface PageOpts {
	limit?: number;
	offset?: number;
}

function pageParams(opts?: PageOpts): Record<string, string | number> {
	const p: Record<string, string | number> = {};
	if (opts?.limit !== undefined) p.limit = opts.limit;
	if (opts?.offset !== undefined) p.offset = opts.offset;
	return p;
}

export function fetchIssues(opts?: PageOpts & { state?: string }): Promise<Page<Issue>> {
	return apiGet<Page<Issue>>('/issues', { state: opts?.state ?? 'open', ...pageParams(opts) });
}

export function fetchPulls(opts?: PageOpts & { state?: string }): Promise<Page<PullRequest>> {
	return apiGet<Page<PullRequest>>('/pulls', { state: opts?.state ?? 'open', ...pageParams(opts) });
}

export function fetchTriage(opts?: PageOpts): Promise<Page<TriageResult>> {
	return apiGet<Page<TriageResult>>('/triage', pageParams(opts));
}

export function fetchQueue(opts?: PageOpts): Promise<Page<QueueEntry>> {
	return apiGet<Page<QueueEntry>>('/queue', pageParams(opts));
}

export function fetchActivity(opts?: PageOpts): Promise<Page<ActivityEntry>> {
	return apiGet<Page<ActivityEntry>>('/activity', pageParams(opts));
}

// ---------------------------------------------------------------------------
// Changelog
// ---------------------------------------------------------------------------

export interface ChangelogPr {
	repo: string;
	number: number;
	title: string;
	author: string | null;
	labels: string[];
	merged_at: string;
}

export interface ChangelogSection {
	name: string;
	pull_requests: ChangelogPr[];
}

export interface ChangelogResult {
	sections: ChangelogSection[];
}

export function fetchChangelog(): Promise<ChangelogResult> {
	return apiGet<ChangelogResult>('/changelog');
}

// ---------------------------------------------------------------------------
// Revert
// ---------------------------------------------------------------------------

export interface RevertRepo {
	repo: string;
	triage_results: number;
	pr_analyses: number;
	labels_to_remove: number;
}

export interface RevertPreview {
	repos: RevertRepo[];
}

export function fetchRevertPreview(): Promise<RevertPreview> {
	return apiGet<RevertPreview>('/revert/preview');
}

// ---------------------------------------------------------------------------
// Backups
// ---------------------------------------------------------------------------

export interface BackupEntry {
	name: string;
	path: string;
	size: number;
	created_at: string;
}

export interface BackupsResult {
	backups: BackupEntry[];
}

export function fetchBackups(): Promise<BackupsResult> {
	return apiGet<BackupsResult>('/backups');
}

export async function createBackup(): Promise<{ status: string; message: string }> {
	const res = await fetch(`${BASE}/backup`, { method: 'POST', headers: CSRF_HEADERS });
	return res.json();
}

export async function restoreBackup(filename: string): Promise<{ status: string; message: string }> {
	const res = await fetch(`${BASE}/restore`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', ...CSRF_HEADERS },
		body: JSON.stringify({ path: filename }),
	});
	return res.json();
}

// ---------------------------------------------------------------------------
// Sync (manual)
// ---------------------------------------------------------------------------

export interface SyncResult {
	status: string;
	synced: string[];
	errors: { repo: string; error: string }[];
}

export async function syncFull(): Promise<SyncResult> {
	const url = new URL('/api/v1/sync/full', window.location.origin);
	const repo = get(selectedRepo);
	if (repo) url.searchParams.set('repo', repo);
	const res = await fetch(url.toString(), { method: 'POST', headers: CSRF_HEADERS });
	return res.json();
}

export async function syncIncremental(): Promise<SyncResult> {
	const url = new URL('/api/v1/sync/incremental', window.location.origin);
	const repo = get(selectedRepo);
	if (repo) url.searchParams.set('repo', repo);
	const res = await fetch(url.toString(), { method: 'POST', headers: CSRF_HEADERS });
	return res.json();
}

// ---------------------------------------------------------------------------
// License
// ---------------------------------------------------------------------------

export interface LicenseFeature {
	id: string;
	label: string;
	enabled: boolean;
}

export interface LicenseInfo {
	is_pro: boolean;
	plan: string;
	features: LicenseFeature[];
	oss_features: string[];
	/** Version of the running daemon (CARGO_PKG_VERSION). Set in
	 *  v0.30.1+; older deploys return undefined and the SPA falls
	 *  back to a generic label. */
	version?: string;
}

export function fetchLicense(): Promise<LicenseInfo> {
	return apiGet<LicenseInfo>('/license');
}

// ---------------------------------------------------------------------------
// Search (Pro feature, gated by license.features `search`)
// ---------------------------------------------------------------------------

export interface SearchHit {
	kind: 'issue' | 'pull' | 'triage' | 'comment';
	repo: string;
	number: number;
	title: string;
	snippet: string;
	updated_at: string;
	rank?: number;
}

export function searchAll(opts: {
	q: string;
	limit?: number;
	offset?: number;
}): Promise<Page<SearchHit>> {
	const params: Record<string, string | number> = { q: opts.q };
	if (opts.limit !== undefined) params.limit = opts.limit;
	if (opts.offset !== undefined) params.offset = opts.offset;
	return apiGet<Page<SearchHit>>('/search', params);
}

export async function activateLicense(key: string): Promise<{ status: string; plan?: string; message: string }> {
	const res = await fetch(`${BASE}/license/activate`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', ...CSRF_HEADERS },
		body: JSON.stringify({ license_key: key }),
	});
	return res.json();
}

// ---------------------------------------------------------------------------
// Repos & Auth (Settings page)
// ---------------------------------------------------------------------------

export interface RepoListEntry {
	slug: string;
	apply: boolean;
	wshm_dir: string;
}

export interface ReposListResponse {
	repos: RepoListEntry[];
	dynamic_add_supported: boolean;
}

export function fetchRepos(): Promise<ReposListResponse> {
	return apiGet<ReposListResponse>('/repos');
}

/** CSRF-defeating header injected on every state-changing request.
 * The daemon rejects POST/PATCH/PUT/DELETE under /api/v1/ that lack
 * this (or X-Requested-With). Browsers cannot set custom headers
 * cross-origin without a CORS preflight that we never grant. */
const CSRF_HEADERS = { 'X-Wshm-Csrf': '1' } as const;

async function apiPost<T>(path: string, body: unknown): Promise<T> {
	const res = await fetch(`${BASE}${path}`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', ...CSRF_HEADERS },
		body: JSON.stringify(body),
	});
	const json = await res.json();
	if (!res.ok) {
		const msg = (json && (json.error || json.message)) || `HTTP ${res.status}`;
		throw new Error(msg);
	}
	return json as T;
}

/** Issue a state-changing fetch with the CSRF header pre-filled. Use
 * for ad-hoc PATCH/PUT/DELETE/POST that don't go through `apiPost`. */
export async function apiMutate(
	path: string,
	init: RequestInit & { method: 'POST' | 'PATCH' | 'PUT' | 'DELETE' },
): Promise<Response> {
	return fetch(`${BASE}${path}`, {
		...init,
		headers: { ...(init.headers ?? {}), ...CSRF_HEADERS },
	});
}

export function addRepo(slug: string, path?: string): Promise<{ status: string; slug: string; path: string; message: string }> {
	return apiPost('/repos', path ? { slug, path } : { slug });
}

export interface AuthStatus {
	github: boolean;
	anthropic: 'oauth' | 'api_key' | null;
}

export function fetchAuthStatus(): Promise<AuthStatus> {
	return apiGet<AuthStatus>('/auth/status');
}

export function setGithubToken(token: string): Promise<{ status: string; message: string; backend?: string }> {
	return apiPost('/auth/github', { token });
}

export function setAnthropicToken(token: string, kind: 'oauth' | 'api_key'): Promise<{ status: string; message: string; backend?: string }> {
	return apiPost('/auth/anthropic', { token, kind });
}

export async function removeGithubToken(): Promise<{ status: string; removed: boolean; message: string }> {
	const res = await fetch(`${BASE}/auth/github`, { method: 'DELETE' });
	const json = await res.json();
	if (!res.ok) throw new Error((json && (json.error || json.message)) || `HTTP ${res.status}`);
	return json;
}

export async function removeAnthropicToken(): Promise<{ status: string; removed: boolean; message: string }> {
	const res = await fetch(`${BASE}/auth/anthropic`, { method: 'DELETE' });
	const json = await res.json();
	if (!res.ok) throw new Error((json && (json.error || json.message)) || `HTTP ${res.status}`);
	return json;
}

export type Role = 'admin' | 'operator' | 'member' | 'viewer';

export interface RepoFilters {
	skip_authors: string[];
	target_branches: string[];
	skip_drafts: boolean;
	triage_only_labels: string[];
	triage_skip_labels: string[];
	triage_max_age_days: number;
	analyze_min_loc: number;
	analyze_max_loc: number;
	auto_pr_only_labels: string[];
	auto_pr_target_branch: string;
	auto_merge_only_authors: string[];
	auto_merge_only_labels: string[];
	auto_merge_min_approvals: number;
	auto_merge_max_loc: number;
}

export interface RepoFeatures {
	collect_issues: boolean;
	collect_prs: boolean;
	triage_issues: boolean;
	analyze_prs: boolean;
	review_prs: boolean;
	auto_pr: boolean;
	auto_merge: boolean;
	filters: RepoFilters;
	/// Master apply-mode toggle. `true` posts to GitHub; `false` is
	/// DRY-RUN (compute results visible in the dashboard, no writes).
	/// Lives on `RepoEntry`, surfaced through the same endpoint for
	/// UI symmetry. Server may include or omit it depending on version.
	apply?: boolean;
}

export async function fetchRepoFeatures(slug: string): Promise<RepoFeatures> {
	return apiGet<RepoFeatures>(`/repos/${encodeURIComponent(slug)}/features`);
}

export async function updateRepoFeatures(
	slug: string,
	patch: Partial<RepoFeatures>
): Promise<RepoFeatures> {
	const res = await fetch(`/api/v1/repos/${encodeURIComponent(slug)}/features`, {
		method: 'PATCH',
		headers: { 'Content-Type': 'application/json', ...CSRF_HEADERS },
		body: JSON.stringify(patch)
	});
	if (!res.ok) {
		const body = await res.json().catch(() => ({}));
		throw new Error(body.error ?? `HTTP ${res.status}`);
	}
	return res.json();
}

/** A configured review "grand domain" (codex, bun, c#…). */
export interface ReviewDomain {
	name: string;
	description?: string | null;
	/** Human-approved. Only validated domains are applied as `domain:*` labels. */
	validated?: boolean;
}
/** Per-repo review-domains config, stored in the DB (works on stateless pods). */
export interface RepoDomains {
	domains: ReviewDomain[];
	review_prompt: string | null;
}

export async function fetchRepoDomains(slug: string): Promise<RepoDomains> {
	const res = await fetch(`/api/v1/repos/${encodeURIComponent(slug)}/domains`);
	if (!res.ok) {
		const body = await res.json().catch(() => ({}));
		throw new Error(body.error ?? `HTTP ${res.status}`);
	}
	return res.json();
}

export async function updateRepoDomains(
	slug: string,
	patch: { domains?: ReviewDomain[]; review_prompt?: string }
): Promise<RepoDomains> {
	const res = await fetch(`/api/v1/repos/${encodeURIComponent(slug)}/domains`, {
		method: 'PATCH',
		headers: { 'Content-Type': 'application/json', ...CSRF_HEADERS },
		body: JSON.stringify(patch)
	});
	if (!res.ok) {
		const body = await res.json().catch(() => ({}));
		throw new Error(body.error ?? `HTTP ${res.status}`);
	}
	return res.json();
}

/** Infer the repo's grand domains (AI) and merge them in as proposed. */
export async function discoverRepoDomains(slug: string): Promise<RepoDomains> {
	const res = await fetch(`/api/v1/repos/${encodeURIComponent(slug)}/domains/discover`, {
		method: 'POST',
		headers: CSRF_HEADERS
	});
	if (!res.ok) {
		const body = await res.json().catch(() => ({}));
		throw new Error(body.error ?? `HTTP ${res.status}`);
	}
	return res.json();
}

/// HTTP retry policy, shared by every outbound call (poller, git
/// providers, AI, self-update). Editable from Settings -> Reliability;
/// changes apply live without a daemon restart.
export interface RetrySettings {
	enabled: boolean;
	max_attempts: number;
	initial_backoff_ms: number;
	max_backoff_ms: number;
}

export function fetchRetrySettings(): Promise<RetrySettings> {
	return apiGet<RetrySettings>('/config/retry');
}

export async function updateRetrySettings(
	patch: Partial<RetrySettings>
): Promise<RetrySettings> {
	const res = await fetch('/api/v1/config/retry', {
		method: 'PATCH',
		headers: { 'Content-Type': 'application/json', ...CSRF_HEADERS },
		body: JSON.stringify(patch)
	});
	if (!res.ok) {
		const body = await res.json().catch(() => ({}));
		throw new Error(body.error ?? `HTTP ${res.status}`);
	}
	return res.json();
}

export interface Me {
	id?: number;
	email: string | null;
	username: string | null;
	role?: Role;
	auth_method: 'sso' | 'local';
}

export function fetchMe(): Promise<Me> {
	return apiGet<Me>('/auth/me');
}

export interface UserRecord {
	id: number;
	email: string;
	username: string | null;
	role: Role;
	sso_provider: string | null;
	created_at: string;
	last_login_at: string | null;
}

export interface UsersListResponse {
	users: UserRecord[];
}

export function fetchUsers(): Promise<UsersListResponse> {
	return apiGet<UsersListResponse>('/users');
}

export async function createUser(payload: {
	email: string;
	username?: string;
	password: string;
	role: Role;
}): Promise<{ id: number }> {
	return apiPost('/users', payload);
}

export async function updateUser(
	id: number,
	payload: { role?: Role; password?: string }
): Promise<{ status: string }> {
	const res = await fetch(`/api/v1/users/${id}`, {
		method: 'PATCH',
		headers: { 'Content-Type': 'application/json', ...CSRF_HEADERS },
		body: JSON.stringify(payload)
	});
	if (!res.ok) throw new Error(await res.text());
	return res.json();
}

export async function deleteUser(id: number): Promise<{ status: string }> {
	const res = await fetch(`/api/v1/users/${id}`, { method: 'DELETE', headers: CSRF_HEADERS });
	if (!res.ok) throw new Error(await res.text());
	return res.json();
}

export interface LogEntry {
	id: number;
	at: string;
	level: 'ERROR' | 'WARN' | 'INFO' | 'DEBUG' | 'TRACE';
	target: string;
	message: string;
}

export interface LogsResponse {
	entries: LogEntry[];
	last_id: number | null;
}

export function fetchLogs(opts: { tail?: number; level?: string; since?: number } = {}): Promise<LogsResponse> {
	// Params go through apiGet's second argument so they get attached via
	// URLSearchParams. Putting them in the path string causes apiGet's
	// `url.pathname = ${BASE}${path}` to percent-encode the `?`, which
	// makes oauth2-proxy hand the request to the SPA fallback (returns
	// HTML and breaks JSON parsing).
	const params: Record<string, string> = {};
	if (opts.tail !== undefined) params.tail = String(opts.tail);
	if (opts.level) params.level = opts.level;
	if (opts.since !== undefined) params.since = String(opts.since);
	return apiGet<LogsResponse>('/logs', params);
}

export interface SecretRecord {
	id: number;
	scope: 'global' | 'repo';
	slug: string | null;
	key: string;
	value: string;     // always "••••••••" except after a reveal
	updated_at: string;
	updated_by: number | null;
}

export function fetchSecrets(): Promise<{ secrets: SecretRecord[] }> {
	return apiGet('/secrets');
}

export function putSecret(input: {
	scope: 'global' | 'repo';
	slug?: string;
	key: string;
	value: string;
}): Promise<{ id: number }> {
	return apiPost('/secrets', input);
}

export async function revealSecret(id: number): Promise<{ value: string }> {
	const res = await fetch(`${BASE}/secrets/${id}/reveal`, { method: 'POST', headers: CSRF_HEADERS });
	const json = await res.json();
	if (!res.ok) throw new Error(json.error ?? `HTTP ${res.status}`);
	return json;
}

export async function deleteSecret(id: number): Promise<{ status: string }> {
	const res = await fetch(`${BASE}/secrets/${id}`, { method: 'DELETE', headers: CSRF_HEADERS });
	const json = await res.json();
	if (!res.ok) throw new Error(json.error ?? `HTTP ${res.status}`);
	return json;
}
