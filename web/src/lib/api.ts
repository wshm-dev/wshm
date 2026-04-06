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
	state: string;
	labels: string[];
	priority: string | null;
	category: string | null;
	created_at: string;
	updated_at: string;
}

export interface PullRequest {
	repo: string;
	number: number;
	title: string;
	state: string;
	labels: string[];
	risk: string | null;
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
