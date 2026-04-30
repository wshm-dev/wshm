import { get } from 'svelte/store';
import { selectedRepo } from './stores';

const BASE = '/api/v1';

function repoParams(): Record<string, string> {
	const repo = get(selectedRepo);
	return repo ? { repo } : {};
}

async function apiGet<T>(path: string, params?: Record<string, string>): Promise<T> {
	const url = new URL(path, window.location.origin);
	url.pathname = `${BASE}${path}`;
	const merged = { ...repoParams(), ...params };
	for (const [key, value] of Object.entries(merged)) {
		url.searchParams.set(key, value);
	}
	const res = await fetch(url.toString());
	if (!res.ok) {
		throw new Error(`API error: ${res.status} ${res.statusText}`);
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
}

export interface PullRequest {
	repo: string;
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
	pr_number: number;
	title: string;
	score: number;
	ci_passing: boolean;
	approvals: number;
	has_conflicts: boolean;
	risk: string | null;
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

export function fetchIssues(state: string = 'open'): Promise<Issue[]> {
	return apiGet<Issue[]>('/issues', { state });
}

export function fetchPulls(state: string = 'open'): Promise<PullRequest[]> {
	return apiGet<PullRequest[]>('/pulls', { state });
}

export function fetchTriage(): Promise<TriageResult[]> {
	return apiGet<TriageResult[]>('/triage');
}

export function fetchQueue(): Promise<QueueEntry[]> {
	return apiGet<QueueEntry[]>('/queue');
}

export function fetchActivity(): Promise<ActivityEntry[]> {
	return apiGet<ActivityEntry[]>('/activity');
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
	const res = await fetch(`${BASE}/backup`, { method: 'POST' });
	return res.json();
}

export async function restoreBackup(filename: string): Promise<{ status: string; message: string }> {
	const res = await fetch(`${BASE}/restore`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
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
	const res = await fetch(url.toString(), { method: 'POST' });
	return res.json();
}

export async function syncIncremental(): Promise<SyncResult> {
	const url = new URL('/api/v1/sync/incremental', window.location.origin);
	const repo = get(selectedRepo);
	if (repo) url.searchParams.set('repo', repo);
	const res = await fetch(url.toString(), { method: 'POST' });
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
}

export function fetchLicense(): Promise<LicenseInfo> {
	return apiGet<LicenseInfo>('/license');
}

export async function activateLicense(key: string): Promise<{ status: string; plan?: string; message: string }> {
	const res = await fetch(`${BASE}/license/activate`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
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

async function apiPost<T>(path: string, body: unknown): Promise<T> {
	const res = await fetch(`${BASE}${path}`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify(body),
	});
	const json = await res.json();
	if (!res.ok) {
		const msg = (json && (json.error || json.message)) || `HTTP ${res.status}`;
		throw new Error(msg);
	}
	return json as T;
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

export function setGithubToken(token: string): Promise<{ status: string; message: string }> {
	return apiPost('/auth/github', { token });
}

export function setAnthropicToken(token: string, kind: 'oauth' | 'api_key'): Promise<{ status: string; message: string }> {
	return apiPost('/auth/anthropic', { token, kind });
}
