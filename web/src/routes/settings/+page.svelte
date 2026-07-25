<script lang="ts">
	import { onMount } from 'svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { Switch } from '$lib/components/ui/switch';
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Alert from '$lib/components/ui/alert';
	import * as Tabs from '$lib/components/ui/tabs';
	import * as RadioGroup from '$lib/components/ui/radio-group';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import { colorConfig, colorPresets, type ColorConfig } from '$lib/colors';
	import { theme } from '$lib/stores';
	import { t, tr } from '$lib/i18n';

	let translate = $state<(k: string) => string>((k) => k);
	t.subscribe((fn) => (translate = fn));
	import {
		fetchLicense,
		activateLicense,
		fetchRepos,
		addRepo,
		fetchAuthStatus,
		setGithubToken,
		setAnthropicToken,
		removeGithubToken,
		removeAnthropicToken,
		fetchSecrets,
		putSecret,
		revealSecret,
		deleteSecret,
		fetchUsers,
		createUser,
		updateUser,
		deleteUser,
		fetchRepoFeatures,
		updateRepoFeatures,
		fetchRepoDomains,
		updateRepoDomains,
		discoverRepoDomains,
		fetchRetrySettings,
		updateRetrySettings,
		type ReviewDomain,
		type RepoFeatures,
		type RetrySettings,
		type LicenseInfo,
		type ReposListResponse,
		type AuthStatus,
		type SecretRecord,
		type UserRecord,
		type Role,
	} from '$lib/api';

	let activeTab: string = $state('repos');

	let colors: ColorConfig = $state({ ...colorConfig.defaults });
	colorConfig.subscribe(c => (colors = { ...c }));

	// License
	let license: LicenseInfo | null = $state(null);
	let licenseKey: string = $state('');
	let activating: boolean = $state(false);
	let activateMessage: string | null = $state(null);
	let activateError: boolean = $state(false);

	// Repositories
	let reposList: ReposListResponse | null = $state(null);
	let newRepoSlug: string = $state('');
	let newRepoPath: string = $state('');
	let addingRepo: boolean = $state(false);
	let addRepoMessage: string | null = $state(null);
	let addRepoError: boolean = $state(false);

	// Auth
	let authStatus: AuthStatus | null = $state(null);
	let ghToken: string = $state('');
	let savingGh: boolean = $state(false);
	let ghMessage: string | null = $state(null);
	let ghError: boolean = $state(false);

	let anthropicToken: string = $state('');
	let anthropicKind: 'oauth' | 'api_key' = $state('oauth');
	let savingAnthropic: boolean = $state(false);
	let anthropicMessage: string | null = $state(null);
	let anthropicError: boolean = $state(false);

	// Encrypted secrets
	let secrets: SecretRecord[] = $state([]);
	let secretsError: string | null = $state(null);
	let newSecretScope: 'global' | 'repo' = $state('global');
	let newSecretSlug: string = $state('');
	let newSecretKey: string = $state('');
	let newSecretValue: string = $state('');
	let savingSecret: boolean = $state(false);
	let secretMessage: string | null = $state(null);
	let secretMessageErr: boolean = $state(false);
	let revealedId: number | null = $state(null);
	let revealedValue: string | null = $state(null);

	async function refreshSecrets() {
		try {
			const r = await fetchSecrets();
			secrets = r.secrets;
			secretsError = null;
		} catch (e) {
			secretsError = e instanceof Error ? e.message : 'load failed';
		}
	}

	async function handleAddSecret() {
		if (!newSecretKey.trim() || !newSecretValue) return;
		savingSecret = true; secretMessage = null; secretMessageErr = false;
		try {
			await putSecret({
				scope: newSecretScope,
				slug: newSecretScope === 'repo' ? newSecretSlug.trim() : undefined,
				key: newSecretKey.trim(),
				value: newSecretValue
			});
			secretMessage = 'Secret saved (encrypted).';
			newSecretKey = ''; newSecretValue = ''; newSecretSlug = '';
			await refreshSecrets();
		} catch (e) {
			secretMessage = e instanceof Error ? e.message : 'save failed';
			secretMessageErr = true;
		}
		savingSecret = false;
	}

	async function handleReveal(id: number) {
		try {
			const r = await revealSecret(id);
			revealedId = id;
			revealedValue = r.value;
			// Auto-hide after 30s
			setTimeout(() => {
				if (revealedId === id) { revealedId = null; revealedValue = null; }
			}, 30000);
		} catch (e) {
			secretsError = e instanceof Error ? e.message : 'reveal failed';
		}
	}

	async function handleDeleteSecret(id: number) {
		try {
			await deleteSecret(id);
			if (revealedId === id) { revealedId = null; revealedValue = null; }
			await refreshSecrets();
		} catch (e) {
			secretsError = e instanceof Error ? e.message : 'delete failed';
		}
	}

	// Users (RBAC)
	let users: UserRecord[] = $state([]);
	let usersError: string | null = $state(null);
	let newUserEmail: string = $state('');
	let newUserUsername: string = $state('');
	let newUserPassword: string = $state('');
	let newUserRole: Role = $state('member');
	let creatingUser: boolean = $state(false);
	let userMessage: string | null = $state(null);
	let userMessageErr: boolean = $state(false);

	// Modal state for Users CRUD
	let createUserModalOpen: boolean = $state(false);
	let editUserModalOpen: boolean = $state(false);
	let deleteUserModalOpen: boolean = $state(false);
	let editingUser: UserRecord | null = $state(null);
	let editRole: Role = $state('member');
	let editPassword: string = $state('');
	let savingEdit: boolean = $state(false);
	let deletingUser: UserRecord | null = $state(null);
	let deletingNow: boolean = $state(false);

	function openCreateUser() {
		newUserEmail = ''; newUserUsername = ''; newUserPassword = ''; newUserRole = 'member';
		userMessage = null; userMessageErr = false;
		createUserModalOpen = true;
	}

	function openEditUser(u: UserRecord) {
		editingUser = u;
		editRole = u.role;
		editPassword = '';
		userMessage = null; userMessageErr = false;
		editUserModalOpen = true;
	}

	function openDeleteUser(u: UserRecord) {
		deletingUser = u;
		deleteUserModalOpen = true;
	}

	async function handleSaveEdit() {
		if (!editingUser) return;
		savingEdit = true;
		try {
			const payload: { role?: Role; password?: string } = {};
			if (editRole !== editingUser.role) payload.role = editRole;
			if (editPassword) {
				if (editPassword.length < 6) {
					userMessage = 'Password must be at least 6 characters';
					userMessageErr = true;
					savingEdit = false;
					return;
				}
				payload.password = editPassword;
			}
			if (Object.keys(payload).length === 0) {
				editUserModalOpen = false;
				savingEdit = false;
				return;
			}
			await updateUser(editingUser.id, payload);
			userMessage = `Updated ${editingUser.email}.`;
			userMessageErr = false;
			editUserModalOpen = false;
			await refreshUsers();
		} catch (e) {
			userMessage = e instanceof Error ? e.message : 'update failed';
			userMessageErr = true;
		}
		savingEdit = false;
	}

	// CSV → trimmed array, drops blanks. Used by the filters UI inputs.
	function parseCsv(s: string): string[] {
		return s.split(',').map((x) => x.trim()).filter((x) => x.length > 0);
	}

	/// Sensible default filter values for repos using GitHub's standard
	/// label set (bug, duplicate, wontfix, invalid, question, good first
	/// issue, help wanted, enhancement, documentation). Applied in one
	/// click via the "Apply GitHub defaults" button in the modal.
	function applyGithubDefaults() {
		if (!featuresDraft) return;
		featuresDraft.filters.skip_authors = [
			'dependabot[bot]',
			'renovate[bot]',
			'github-actions[bot]'
		];
		featuresDraft.filters.triage_skip_labels = [
			'wontfix',
			'duplicate',
			'invalid',
			'question'
		];
		featuresDraft.filters.auto_pr_only_labels = ['good first issue', 'help wanted'];
		featuresDraft.filters.auto_merge_only_authors = ['dependabot[bot]'];
		featuresDraft.filters.auto_merge_only_labels = ['auto-merge'];
		featuresDraft.filters.auto_merge_max_loc = 200;
		featuresDraft.filters.skip_drafts = true;
	}

	// Repo features (per-repo toggles)
	let featuresModalOpen: boolean = $state(false);
	let featuresSlug: string = $state('');
	let featuresDraft: RepoFeatures | null = $state(null);
	let featuresSaving: boolean = $state(false);
	let featuresMessage: string | null = $state(null);
	let featuresMessageErr: boolean = $state(false);

	// Review "grand domains" (codex, bun, …) + prompt. DB-backed (works on
	// stateless pods). Managed in the dedicated "Domains" tab, per repo.
	let domainsRepo: string = $state('');
	let domainsDraft: ReviewDomain[] = $state([]);
	let reviewPromptDraft: string = $state('');
	let domainsLoading: boolean = $state(false);
	let domainsSaving: boolean = $state(false);
	let domainsMessage: string | null = $state(null);
	let domainsMessageErr: boolean = $state(false);

	async function loadDomainsFor(slug: string) {
		domainsRepo = slug;
		domainsDraft = [];
		reviewPromptDraft = '';
		domainsMessage = null;
		if (!slug) return;
		domainsLoading = true;
		try {
			const d = await fetchRepoDomains(slug);
			domainsDraft = d.domains ?? [];
			reviewPromptDraft = d.review_prompt ?? '';
		} catch (e) {
			domainsMessage = e instanceof Error ? e.message : 'Failed to load domains';
			domainsMessageErr = true;
		}
		domainsLoading = false;
	}
	async function saveDomains() {
		if (!domainsRepo) return;
		domainsSaving = true;
		domainsMessage = null;
		try {
			await updateRepoDomains(domainsRepo, {
				domains: domainsDraft.filter((d) => d.name.trim() !== ''),
				review_prompt: reviewPromptDraft
			});
			domainsMessage = 'Saved.';
			domainsMessageErr = false;
		} catch (e) {
			domainsMessage = e instanceof Error ? e.message : 'save failed';
			domainsMessageErr = true;
		}
		domainsSaving = false;
	}

	async function openFeaturesModal(slug: string) {
		featuresSlug = slug;
		featuresDraft = null;
		featuresMessage = null;
		featuresMessageErr = false;
		featuresModalOpen = true;
		try {
			featuresDraft = await fetchRepoFeatures(slug);
		} catch (e) {
			featuresMessage = e instanceof Error ? e.message : 'Failed to load features';
			featuresMessageErr = true;
		}
	}

	let domainsDiscovering: boolean = $state(false);
	function addDomain() {
		domainsDraft = [...domainsDraft, { name: '', description: '', validated: true }];
	}
	function removeDomain(i: number) {
		domainsDraft = domainsDraft.filter((_, idx) => idx !== i);
	}
	function validateAllDomains() {
		domainsDraft = domainsDraft.map((d) => ({ ...d, validated: true }));
	}
	async function handleDiscoverDomains() {
		if (!domainsRepo) return;
		domainsDiscovering = true;
		domainsMessage = null;
		try {
			const d = await discoverRepoDomains(domainsRepo);
			domainsDraft = d.domains ?? [];
			reviewPromptDraft = d.review_prompt ?? reviewPromptDraft;
		} catch (e) {
			domainsMessage = e instanceof Error ? e.message : 'discovery failed';
			domainsMessageErr = true;
		}
		domainsDiscovering = false;
	}

	async function handleSaveFeatures() {
		if (!featuresDraft) return;
		featuresSaving = true;
		try {
			await updateRepoFeatures(featuresSlug, featuresDraft);
			featuresMessage = tr('settings.features.saved');
			featuresMessageErr = false;
			featuresModalOpen = false;
			await refreshRepos();
		} catch (e) {
			featuresMessage = e instanceof Error ? e.message : 'save failed';
			featuresMessageErr = true;
		}
		featuresSaving = false;
	}

	async function handleConfirmDelete() {
		if (!deletingUser) return;
		deletingNow = true;
		try {
			await deleteUser(deletingUser.id);
			userMessage = `Deleted ${deletingUser.email}.`;
			userMessageErr = false;
			deleteUserModalOpen = false;
			deletingUser = null;
			await refreshUsers();
		} catch (e) {
			userMessage = e instanceof Error ? e.message : 'delete failed';
			userMessageErr = true;
		}
		deletingNow = false;
	}

	async function refreshUsers() {
		try {
			const r = await fetchUsers();
			users = r.users;
			usersError = null;
		} catch (e) {
			usersError = e instanceof Error ? e.message : 'load failed';
		}
	}

	async function handleCreateUser() {
		if (!newUserEmail.trim() || !newUserPassword) return;
		creatingUser = true; userMessage = null; userMessageErr = false;
		try {
			await createUser({
				email: newUserEmail.trim(),
				username: newUserUsername.trim() || undefined,
				password: newUserPassword,
				role: newUserRole,
			});
			userMessage = `User ${newUserEmail} created.`;
			createUserModalOpen = false;
			await refreshUsers();
		} catch (e) {
			userMessage = e instanceof Error ? e.message : 'create failed';
			userMessageErr = true;
		}
		creatingUser = false;
	}

	function saveColors() { colorConfig.save(colors); }
	function resetColors() { colorConfig.reset(); colors = { ...colorConfig.defaults }; }
	function applyPreset(id: string) {
		const preset = colorPresets[id];
		if (preset) { colorConfig.save({ ...preset }); colors = { ...preset }; }
	}
	const presetLabels: Record<string, string> = {
		default: 'settings.appearance.presetDefault',
		highContrast: 'settings.appearance.presetHighContrast',
		muted: 'settings.appearance.presetMuted'
	};

	async function refreshRepos() {
		try { reposList = await fetchRepos(); } catch { /* ignore */ }
	}
	async function refreshAuth() {
		try { authStatus = await fetchAuthStatus(); } catch { /* ignore */ }
	}

	async function handleAddRepo() {
		if (!newRepoSlug.trim()) return;
		addingRepo = true; addRepoMessage = null; addRepoError = false;
		try {
			const r = await addRepo(newRepoSlug.trim(), newRepoPath.trim() || undefined);
			addRepoMessage = r.message;
			newRepoSlug = ''; newRepoPath = '';
			await refreshRepos();
		} catch (e) {
			addRepoMessage = e instanceof Error ? e.message : 'Add failed';
			addRepoError = true;
		}
		addingRepo = false;
	}

	async function handleSetGithub() {
		if (!ghToken.trim()) return;
		savingGh = true; ghMessage = null; ghError = false;
		try {
			const r = await setGithubToken(ghToken.trim());
			ghMessage = r.message; ghToken = '';
			await refreshAuth();
		} catch (e) {
			ghMessage = e instanceof Error ? e.message : 'Save failed';
			ghError = true;
		}
		savingGh = false;
	}

	async function handleSetAnthropic() {
		if (!anthropicToken.trim()) return;
		savingAnthropic = true; anthropicMessage = null; anthropicError = false;
		try {
			const r = await setAnthropicToken(anthropicToken.trim(), anthropicKind);
			anthropicMessage = r.message; anthropicToken = '';
			await refreshAuth();
		} catch (e) {
			anthropicMessage = e instanceof Error ? e.message : 'Save failed';
			anthropicError = true;
		}
		savingAnthropic = false;
	}

	async function handleRemoveGithub() {
		if (!confirm('Remove the GitHub token from this wshm instance?')) return;
		savingGh = true; ghMessage = null; ghError = false;
		try {
			const r = await removeGithubToken();
			ghMessage = r.message;
			await refreshAuth();
		} catch (e) {
			ghMessage = e instanceof Error ? e.message : 'Remove failed';
			ghError = true;
		}
		savingGh = false;
	}

	async function handleRemoveAnthropic() {
		if (!confirm('Remove the Anthropic credentials from this wshm instance?')) return;
		savingAnthropic = true; anthropicMessage = null; anthropicError = false;
		try {
			const r = await removeAnthropicToken();
			anthropicMessage = r.message;
			await refreshAuth();
		} catch (e) {
			anthropicMessage = e instanceof Error ? e.message : 'Remove failed';
			anthropicError = true;
		}
		savingAnthropic = false;
	}

	async function handleActivate() {
		if (!licenseKey.trim()) return;
		activating = true; activateMessage = null; activateError = false;
		try {
			const r = await activateLicense(licenseKey.trim());
			if (r.status === 'ok') {
				activateMessage = r.message; activateError = false; licenseKey = '';
				license = await fetchLicense();
			} else {
				activateMessage = r.message; activateError = true;
			}
		} catch (e) {
			activateMessage = e instanceof Error ? e.message : 'Activation failed';
			activateError = true;
		}
		activating = false;
	}

	// Retry policy (Reliability tab)
	let retrySettings: RetrySettings | null = $state(null);
	let savingRetry: boolean = $state(false);
	let retryMessage: string | null = $state(null);
	let retryError: boolean = $state(false);

	async function handleSaveRetry() {
		if (!retrySettings) return;
		savingRetry = true;
		retryMessage = null;
		try {
			retrySettings = await updateRetrySettings(retrySettings);
			retryError = false;
			retryMessage = translate('settings.retry.saved');
		} catch (e) {
			retryError = true;
			retryMessage = e instanceof Error ? e.message : String(e);
		}
		savingRetry = false;
	}

	onMount(async () => {
		try { license = await fetchLicense(); } catch { /* ignore */ }
		try { retrySettings = await fetchRetrySettings(); } catch { /* ignore */ }
		await refreshRepos();
		await refreshAuth();
		await refreshSecrets();
		await refreshUsers();
	});
</script>

<!--
	Reusable info bubble: a small "?" badge next to a label that shows a
	hover/focus tooltip with deeper context. Use for options whose name
	doesn't fully convey *what wshm actually does* when toggled on.
	`bodyKey` is an i18n key resolved via `$t` (en/fr translations live
	in src/lib/i18n/{en,fr}.json; other locales fall back to English).
-->
{#snippet infoTip(id: string, bodyKey: string)}
	<Tooltip.Provider>
		<Tooltip.Root>
			<Tooltip.Trigger
				{id}
				aria-label={$t('common.moreInfo')}
				class="inline-flex items-center justify-center w-4 h-4 rounded-full bg-muted text-foreground/90 text-[10px] font-bold hover:bg-primary hover:text-primary-foreground transition-colors cursor-help"
			>?</Tooltip.Trigger>
			<Tooltip.Content side="right" class="max-w-xs text-xs leading-snug">
				{$t(bodyKey)}
			</Tooltip.Content>
		</Tooltip.Root>
	</Tooltip.Provider>
{/snippet}

<!-- Reusable success/error feedback alert. -->
{#snippet statusAlert(message: string, isError: boolean, cls: string = '')}
	<Alert.Root
		variant={isError ? 'destructive' : 'default'}
		class="py-2 {isError ? '' : 'border-green-500/30 bg-green-500/10'} {cls}"
	>
		<Alert.Description class="text-xs {isError ? '' : 'text-green-600 dark:text-green-400'}">
			{message}
		</Alert.Description>
	</Alert.Root>
{/snippet}

<svelte:head>
	<title>wshm - Settings</title>
</svelte:head>

<div class="mb-4">
	<h2 class="text-xl font-bold tracking-tight mb-1">{$t('settings.title')}</h2>
	<p class="text-sm text-muted-foreground">{$t('settings.subtitle')}</p>
</div>

<Tabs.Root bind:value={activeTab}>
	<Tabs.List class="h-auto flex-wrap">
		<Tabs.Trigger value="repos">{$t('settings.tabs.repos')}</Tabs.Trigger>
		<Tabs.Trigger value="git-providers">{$t('settings.tabs.gitProviders')}</Tabs.Trigger>
		<Tabs.Trigger value="ai-providers">{$t('settings.tabs.aiProviders')}</Tabs.Trigger>
		<Tabs.Trigger value="license">{$t('settings.tabs.license')}</Tabs.Trigger>
		<Tabs.Trigger value="appearance">{$t('settings.tabs.appearance')}</Tabs.Trigger>
		<Tabs.Trigger value="configuration">{$t('settings.tabs.configuration')}</Tabs.Trigger>
		<Tabs.Trigger value="reliability">{$t('settings.tabs.reliability')}</Tabs.Trigger>
		<Tabs.Trigger value="secrets">{$t('settings.tabs.secrets')}</Tabs.Trigger>
		<Tabs.Trigger value="users">{$t('settings.tabs.users')}</Tabs.Trigger>
		<Tabs.Trigger value="domains">Domains</Tabs.Trigger>
	</Tabs.List>

	<!-- ========================= REPOSITORIES ========================= -->
	<Tabs.Content value="repos" class="mt-2">
		<div class="w-full">
			<Card.Root>
				<Card.Header>
					<Card.Title>{$t('settings.repos.title')}</Card.Title>
				</Card.Header>
				<Card.Content>
					{#if reposList}
						<div class="mb-3">
							<h4 class="text-xs font-semibold text-primary mb-2">{$t('settings.repos.configured')} ({reposList.repos.length})</h4>
							{#if reposList.repos.length === 0}
								<p class="text-xs text-muted-foreground">{$t('settings.repos.none')}</p>
							{:else}
								<ul class="space-y-1 text-xs">
									{#each reposList.repos as r}
										<li class="flex items-center justify-between gap-2">
											<span class="text-foreground/90 mono">{r.slug}</span>
											<div class="flex items-center gap-2">
												<Badge
													variant={r.apply ? 'outline' : 'secondary'}
													class={r.apply ? 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400' : ''}
												>{r.apply ? $t('settings.repos.badge.apply') : $t('settings.repos.badge.dryrun')}</Badge>
												<Button variant="outline" size="xs" onclick={() => openFeaturesModal(r.slug)}>
													{$t('settings.repos.editFeatures')}
												</Button>
											</div>
										</li>
									{/each}
								</ul>
							{/if}
						</div>

						<div class="border-t pt-3 space-y-2">
							{#if addRepoMessage}
								{@render statusAlert(addRepoMessage, addRepoError)}
							{/if}

							{#if reposList.dynamic_add_supported}
								<form onsubmit={(e) => { e.preventDefault(); handleAddRepo(); }} class="space-y-2">
									<div>
										<Label for="repo-slug" class="text-xs mb-1">{$t('settings.repos.slug')}</Label>
										<Input id="repo-slug" type="text" bind:value={newRepoSlug} placeholder="owner/repo" disabled={addingRepo} class="h-8" />
									</div>
									<div>
										<Label for="repo-path" class="text-xs mb-1">{$t('settings.repos.pathOptional')}</Label>
										<Input id="repo-path" type="text" bind:value={newRepoPath} placeholder="/abs/path" disabled={addingRepo} class="h-8" />
									</div>
									<Button type="submit" disabled={addingRepo || !newRepoSlug.trim()} size="sm" class="w-full">
										{addingRepo ? $t('settings.repos.adding') : $t('settings.repos.add')}
									</Button>
								</form>
							{:else}
								<p class="text-xs text-muted-foreground">
									{$t('settings.repos.dynamicNotAvailable')}
									<code class="rounded bg-muted px-1 py-0.5">~/.wshm/global.toml</code>
									{$t('settings.repos.dynamicNotAvailable.suffix')}
								</p>
							{/if}
						</div>
					{:else}
						<p class="text-sm text-muted-foreground">{$t('common.loading')}</p>
					{/if}
				</Card.Content>
			</Card.Root>
		</div>
	</Tabs.Content>

	<!-- ========================= DOMAINS ========================= -->
	<!-- Review "grand domains" — DB-backed (app_settings), so they survive on
	     stateless pods and are shared across replicas. Discovered per repo, then
	     validated → applied as `domain:*` GitHub labels by the review pipeline. -->
	<Tabs.Content value="domains" class="mt-2">
		<div class="w-full">
			<Card.Root>
				<Card.Header>
					<Card.Title>Review domains</Card.Title>
					<Card.Description class="text-xs">
						Broad areas the AI review tags each PR/issue with (codex, bun, c#…).
						<strong>Discover</strong> infers them from the repo; only <strong>validated</strong> (✓)
						domains are applied as <code>domain:*</code> labels — proposed ones wait for your review.
						Filterable in the Graphs page.
					</Card.Description>
				</Card.Header>
				<Card.Content class="space-y-3">
					<div>
						<Label class="text-xs mb-1">Repository</Label>
						<select
							class="w-full rounded-md border bg-background px-2 py-1.5 text-xs"
							value={domainsRepo}
							onchange={(e) => loadDomainsFor((e.currentTarget as HTMLSelectElement).value)}
						>
							<option value="" disabled>Select a repository…</option>
							{#each reposList?.repos ?? [] as r}
								<option value={r.slug}>{r.slug}</option>
							{/each}
						</select>
					</div>

					{#if domainsMessage}
						{@render statusAlert(domainsMessage, domainsMessageErr)}
					{/if}

					{#if domainsRepo}
						{#if domainsLoading}
							<p class="text-xs text-muted-foreground">{$t('common.loading')}</p>
						{:else}
							<div class="flex items-center justify-between">
								<h5 class="text-xs uppercase text-muted-foreground font-semibold">Domains</h5>
								<div class="flex items-center gap-2">
									<button
										type="button"
										class="text-xs text-primary hover:underline disabled:opacity-50"
										onclick={handleDiscoverDomains}
										disabled={domainsDiscovering}
									>
										{domainsDiscovering ? 'discovering…' : 'discover'}
									</button>
									<button type="button" class="text-xs text-primary hover:underline" onclick={validateAllDomains}>
										validate all
									</button>
									<button type="button" class="text-xs text-primary hover:underline" onclick={addDomain}>
										+ add
									</button>
								</div>
							</div>
							{#each domainsDraft as d, i}
								<div class="flex items-center gap-1">
									<input
										type="checkbox"
										class="h-4 w-4 shrink-0"
										checked={d.validated ?? false}
										onchange={(e) => (d.validated = (e.currentTarget as HTMLInputElement).checked)}
										title="validated → applied as a GitHub label"
									/>
									<Input class="h-8 w-1/3" placeholder="name" bind:value={d.name} />
									<Badge
										variant="secondary"
										class="shrink-0 tabular-nums"
										title="pull requests + issues grouped under this domain"
									>
										{d.count ?? 0} PR
									</Badge>
									<Input
										class="h-8 flex-1"
										placeholder="description (helps the AI decide)"
										value={d.description ?? ''}
										oninput={(e) => (d.description = (e.currentTarget as HTMLInputElement).value)}
									/>
									<button
										type="button"
										class="px-2 text-muted-foreground hover:text-destructive"
										onclick={() => removeDomain(i)}
										aria-label="remove domain"
									>
										✕
									</button>
								</div>
							{:else}
								<p class="text-[0.7rem] text-muted-foreground">
									No domains yet — click <strong>discover</strong> to infer them from the repo.
								</p>
							{/each}

							<div>
								<Label class="text-xs mb-1 mt-2">Custom review prompt (optional)</Label>
								<textarea
									class="w-full rounded-md border bg-background px-2 py-1 text-xs min-h-[64px]"
									placeholder="Overrides the default domain instruction sent to the AI. Leave empty to use the built-in one."
									bind:value={reviewPromptDraft}
								></textarea>
							</div>

							<Button size="sm" onclick={saveDomains} disabled={domainsSaving}>
								{domainsSaving ? $t('common.loading') : $t('common.save')}
							</Button>
						{/if}
					{:else}
						<p class="text-xs text-muted-foreground">Select a repository to manage its review domains.</p>
					{/if}
				</Card.Content>
			</Card.Root>
		</div>
	</Tabs.Content>

	<!-- ========================= GIT PROVIDERS ========================= -->
	<Tabs.Content value="git-providers" class="mt-2">
		<div class="w-full">
			<Card.Root>
				<Card.Header>
					<Card.Title>{$t('settings.git.title')}</Card.Title>
				</Card.Header>
				<Card.Content>
					{#if authStatus}
						<div class="mb-3">
							<Badge
								variant={authStatus.github ? 'outline' : 'secondary'}
								class={authStatus.github ? 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400' : ''}
							>
								{authStatus.github ? $t('settings.git.configured') : $t('settings.git.notConfigured')}
							</Badge>
						</div>

						<p class="text-xs text-muted-foreground mb-3">
							{$t('settings.git.helper.intro')} <a href="https://github.com/settings/tokens" target="_blank" class="text-primary hover:underline">{$t('settings.git.helper.generate')}</a> {$t('settings.git.helper.scope')}
						</p>

						{#if ghMessage}
							{@render statusAlert(ghMessage, ghError, 'mb-2')}
						{/if}

						<form onsubmit={(e) => { e.preventDefault(); handleSetGithub(); }} class="space-y-2">
							<div>
								<Label for="gh-token" class="text-xs mb-1">{$t('settings.git.token')}</Label>
								<Input id="gh-token" type="password" bind:value={ghToken} placeholder="ghp_..." disabled={savingGh} class="h-8" />
							</div>
							<Button type="submit" disabled={savingGh || !ghToken.trim()} size="sm" class="w-full">
								{savingGh ? $t('common.saving') : $t('settings.git.save')}
							</Button>
							{#if authStatus.github}
								<Button type="button" variant="destructive" disabled={savingGh} size="sm" class="w-full" onclick={handleRemoveGithub}>
									{$t('settings.git.remove')}
								</Button>
							{/if}
						</form>
					{:else}
						<p class="text-sm text-muted-foreground">{$t('common.loading')}</p>
					{/if}
				</Card.Content>
			</Card.Root>
			<p class="mt-3 text-xs text-muted-foreground">{$t('settings.git.moreSoon')}</p>
		</div>
	</Tabs.Content>

	<!-- ========================= AI PROVIDERS ========================= -->
	<Tabs.Content value="ai-providers" class="mt-2">
		<div class="w-full">
			<Card.Root>
				<Card.Header>
					<Card.Title>{$t('settings.ai.title')}</Card.Title>
				</Card.Header>
				<Card.Content>
					{#if authStatus}
						<div class="mb-3">
							<Badge
								variant={authStatus.anthropic ? 'outline' : 'secondary'}
								class={authStatus.anthropic ? 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400' : ''}
							>
								{authStatus.anthropic === 'oauth'
									? $t('settings.ai.badge.oauth')
									: authStatus.anthropic === 'api_key'
										? $t('settings.ai.badge.apiKey')
										: $t('settings.ai.badge.notConfigured')}
							</Badge>
						</div>

						<p class="text-xs text-muted-foreground mb-3">
							{$t('settings.ai.helper')} <code class="rounded bg-muted px-1 py-0.5">claude /token</code> {$t('settings.ai.helper.suffix')}
							<a href="https://console.anthropic.com/" target="_blank" class="text-primary hover:underline">{$t('settings.ai.helper.apiKey')}</a>.
						</p>

						{#if anthropicMessage}
							{@render statusAlert(anthropicMessage, anthropicError, 'mb-2')}
						{/if}

						<form onsubmit={(e) => { e.preventDefault(); handleSetAnthropic(); }} class="space-y-2">
							<RadioGroup.Root
								value={anthropicKind}
								onValueChange={(v) => (anthropicKind = v as 'oauth' | 'api_key')}
								disabled={savingAnthropic}
								class="flex gap-4"
							>
								<div class="flex items-center gap-2">
									<RadioGroup.Item value="oauth" id="anth-kind-oauth" />
									<Label for="anth-kind-oauth" class="text-xs font-normal">{$t('settings.ai.kind.oauth')}</Label>
								</div>
								<div class="flex items-center gap-2">
									<RadioGroup.Item value="api_key" id="anth-kind-api-key" />
									<Label for="anth-kind-api-key" class="text-xs font-normal">{$t('settings.ai.kind.apiKey')}</Label>
								</div>
							</RadioGroup.Root>
							<div>
								<Label for="anth-token" class="text-xs mb-1">{$t('settings.git.token')}</Label>
								<Input id="anth-token" type="password" bind:value={anthropicToken} placeholder={anthropicKind === 'oauth' ? 'sk-ant-oat01-...' : 'sk-ant-api03-...'} disabled={savingAnthropic} class="h-8" />
							</div>
							<Button type="submit" disabled={savingAnthropic || !anthropicToken.trim()} size="sm" class="w-full">
								{savingAnthropic ? $t('common.saving') : $t('settings.ai.save')}
							</Button>
							{#if authStatus.anthropic}
								<Button type="button" variant="destructive" disabled={savingAnthropic} size="sm" class="w-full" onclick={handleRemoveAnthropic}>
									{$t('settings.ai.remove')}
								</Button>
							{/if}
						</form>
					{:else}
						<p class="text-sm text-muted-foreground">{$t('common.loading')}</p>
					{/if}
				</Card.Content>
			</Card.Root>
			<p class="mt-3 text-xs text-muted-foreground">{$t('settings.ai.moreSoon')}</p>
		</div>
	</Tabs.Content>

	<!-- ============================ LICENSE ============================ -->
	<Tabs.Content value="license" class="mt-2">
		<Card.Root>
			<Card.Header>
				<Card.Title>{$t('settings.license.title')}</Card.Title>
			</Card.Header>
			<Card.Content>
				{#if license}
					<div class="flex items-center gap-3 mb-4">
						<Badge
							variant={license.is_pro ? 'outline' : 'secondary'}
							class={license.is_pro ? 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400' : ''}
						>{license.plan.toUpperCase()}</Badge>
						{#if !license.is_pro}
							<span class="text-sm text-muted-foreground">{$t('settings.license.free')}</span>
						{/if}
					</div>

					<div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-4">
						<div>
							<h4 class="text-sm font-semibold text-muted-foreground mb-2">{$t('settings.license.ossFeatures')}</h4>
							<div class="flex flex-wrap gap-1">
								{#each license.oss_features as f}
									<Badge variant="outline" class="bg-primary/15 text-primary">{f}</Badge>
								{/each}
							</div>
						</div>
						<div>
							<h4 class="text-sm font-semibold text-muted-foreground mb-2">{$t('settings.license.proFeatures')}</h4>
							<div class="space-y-1">
								{#each license.features as f}
									<div class="flex items-center justify-between text-sm">
										<span class="text-foreground/90">{f.label}</span>
										<Badge
											variant={f.enabled ? 'outline' : 'secondary'}
											class={f.enabled ? 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400' : ''}
										>{f.enabled ? $t('settings.license.feature.active') : $t('settings.license.feature.locked')}</Badge>
									</div>
								{/each}
							</div>
						</div>
					</div>

					<div class="border-t pt-3">
						{#if activateMessage}
							{@render statusAlert(activateMessage, activateError, 'mb-2')}
						{/if}

						<p class="text-xs text-muted-foreground mb-2">{license.is_pro ? $t('settings.license.update') : $t('settings.license.enter')}</p>
						<form onsubmit={(e) => { e.preventDefault(); handleActivate(); }} class="flex gap-2">
							<Input type="text" bind:value={licenseKey} placeholder="wshm-pro-xxxx-xxxx-xxxx" disabled={activating} class="h-8 flex-1" />
							<Button type="submit" disabled={activating || !licenseKey.trim()} size="sm">
								{activating ? $t('settings.license.activating') : $t('settings.license.activate')}
							</Button>
						</form>

						{#if !license.is_pro}
							<p class="text-xs text-muted-foreground mt-2">
								<a href="https://wshm.dev/pro" target="_blank" class="text-primary hover:underline">{$t('settings.license.getLicense')}</a>
							</p>
						{/if}
					</div>
				{:else}
					<p class="text-sm text-muted-foreground">{$t('common.loading')}</p>
				{/if}
			</Card.Content>
		</Card.Root>
	</Tabs.Content>

	<!-- ========================== APPEARANCE ========================== -->
	<Tabs.Content value="appearance" class="mt-2">
		<Card.Root class="mb-4">
			<Card.Content>
				<div class="grid grid-cols-1 sm:grid-cols-2 gap-6">
					<div>
						<h3 class="text-base font-medium mb-3">{$t('settings.appearance.theme')}</h3>
						<div class="flex gap-2">
							<Button size="xs" variant={$theme === 'dark' ? 'default' : 'outline'} onclick={() => theme.set('dark')}>
								{$t('settings.appearance.themeDark')}
							</Button>
							<Button size="xs" variant={$theme === 'light' ? 'default' : 'outline'} onclick={() => theme.set('light')}>
								{$t('settings.appearance.themeLight')}
							</Button>
						</div>
					</div>
					<div>
						<h3 class="text-base font-medium mb-3">{$t('settings.appearance.presets')}</h3>
						<div class="flex gap-2 flex-wrap">
							{#each Object.keys(colorPresets) as id}
								<button
									type="button"
									class="flex items-center gap-2 px-3 py-1.5 rounded border bg-background text-xs text-foreground/90 hover:border-muted-foreground"
									onclick={() => applyPreset(id)}
								>
									{$t(presetLabels[id])}
									<span class="flex gap-0.5">
										{#each ['critical', 'high', 'medium', 'feature'] as k}
											<span class="w-2 h-2 rounded-full inline-block" style="background: {colorPresets[id][k]}"></span>
										{/each}
									</span>
								</button>
							{/each}
						</div>
					</div>
				</div>
			</Card.Content>
		</Card.Root>
		<div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
			<Card.Root>
				<Card.Header>
					<Card.Title>{$t('settings.appearance.colorScheme')}</Card.Title>
				</Card.Header>
				<Card.Content>
					<div class="mb-3 border-b pb-3">
						<h4 class="text-xs font-semibold text-primary mb-2">{$t('settings.appearance.issuePrStatus')}</h4>
						<div class="space-y-1.5">
							{#each [['noPr', $t('settings.appearance.noPr')], ['hasPr', $t('settings.appearance.hasPr')], ['prReady', $t('settings.appearance.prReady')]] as [key, label]}
								<label class="flex items-center gap-2">
									<input type="color" bind:value={colors[key]} onchange={saveColors} class="w-6 h-5 rounded border bg-transparent cursor-pointer" />
									<span class="text-xs text-foreground/90">{label}</span>
									<span class="ml-auto text-[0.6rem] mono text-muted-foreground">{colors[key]}</span>
								</label>
							{/each}
						</div>
					</div>

					<div class="mb-3 border-b pb-3">
						<h4 class="text-xs font-semibold text-primary mb-2">{$t('settings.appearance.priority')}</h4>
						<div class="space-y-1.5">
							{#each [['critical', $t('settings.appearance.critical')], ['high', $t('settings.appearance.high')], ['medium', $t('settings.appearance.medium')], ['low', $t('settings.appearance.low')]] as [key, label]}
								<label class="flex items-center gap-2">
									<input type="color" bind:value={colors[key]} onchange={saveColors} class="w-6 h-5 rounded border bg-transparent cursor-pointer" />
									<span class="text-xs text-foreground/90">{label}</span>
									<span class="ml-auto text-[0.6rem] mono text-muted-foreground">{colors[key]}</span>
								</label>
							{/each}
						</div>
					</div>

					<div class="mb-3 border-b pb-3">
						<h4 class="text-xs font-semibold text-primary mb-2">{$t('settings.appearance.riskCategory')}</h4>
						<div class="space-y-1.5">
							{#each [['riskHigh', $t('settings.appearance.riskHigh')], ['riskMedium', $t('settings.appearance.riskMedium')], ['riskLow', $t('settings.appearance.riskLow')], ['bug', $t('settings.appearance.bug')], ['feature', $t('settings.appearance.feature')], ['docs', $t('settings.appearance.docs')]] as [key, label]}
								<label class="flex items-center gap-2">
									<input type="color" bind:value={colors[key]} onchange={saveColors} class="w-6 h-5 rounded border bg-transparent cursor-pointer" />
									<span class="text-xs text-foreground/90">{label}</span>
									<span class="ml-auto text-[0.6rem] mono text-muted-foreground">{colors[key]}</span>
								</label>
							{/each}
						</div>
					</div>

					<Button onclick={resetColors} variant="outline" size="xs">{$t('settings.appearance.reset')}</Button>
				</Card.Content>
			</Card.Root>

			<Card.Root>
				<Card.Header>
					<Card.Title>{$t('settings.appearance.legend')}</Card.Title>
				</Card.Header>
				<Card.Content>
					<div class="grid grid-cols-2 gap-4 text-xs text-foreground/90">
						<div>
							<h4 class="text-muted-foreground mb-1 text-[0.6rem] uppercase">{$t('settings.appearance.legend.prStatus')}</h4>
							{#each [['noPr', $t('settings.appearance.noPr')], ['hasPr', $t('settings.appearance.hasPr')], ['prReady', $t('settings.appearance.prReady')]] as [key, label]}
								<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded inline-block" style="background: {colors[key]}"></span> {label}</div>
							{/each}
						</div>
						<div>
							<h4 class="text-muted-foreground mb-1 text-[0.6rem] uppercase">{$t('settings.appearance.legend.priority')}</h4>
							{#each [['critical', $t('settings.appearance.critical')], ['high', $t('settings.appearance.high')], ['medium', $t('settings.appearance.medium')], ['low', $t('settings.appearance.low')]] as [key, label]}
								<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded inline-block" style="background: {colors[key]}"></span> {label}</div>
							{/each}
						</div>
						<div>
							<h4 class="text-muted-foreground mb-1 text-[0.6rem] uppercase">{$t('settings.appearance.legend.risk')}</h4>
							{#each [['riskHigh', $t('settings.appearance.high')], ['riskMedium', $t('settings.appearance.medium')], ['riskLow', $t('settings.appearance.low')]] as [key, label]}
								<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded inline-block" style="background: {colors[key]}"></span> {label}</div>
							{/each}
						</div>
						<div>
							<h4 class="text-muted-foreground mb-1 text-[0.6rem] uppercase">{$t('settings.appearance.legend.category')}</h4>
							{#each [['bug', $t('settings.appearance.bug')], ['feature', $t('settings.appearance.feature')], ['docs', $t('settings.appearance.docs')]] as [key, label]}
								<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded inline-block" style="background: {colors[key]}"></span> {label}</div>
							{/each}
						</div>
					</div>
				</Card.Content>
			</Card.Root>
		</div>
	</Tabs.Content>

	<!-- ========================= CONFIGURATION ========================= -->
	<Tabs.Content value="configuration" class="mt-2">
		<Card.Root>
			<Card.Header>
				<Card.Title>{$t('settings.config.title')}</Card.Title>
				<Card.Description class="text-xs">
					{$t('settings.config.helper.prefix')} <code class="rounded bg-muted px-1 py-0.5">.wshm/config.toml</code>{$t('settings.config.helper.suffix')}
				</Card.Description>
			</Card.Header>
			<Card.Content>
				<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
					{#each [
						[$t('settings.config.section.triage'), [['Enabled', 'true'], ['Auto-fix', 'false'], ['Confidence', '0.85']]],
						[$t('settings.config.section.prAnalysis'), [['Enabled', 'true'], ['Auto-label', 'true'], ['Risk labels', 'true']]],
						[$t('settings.config.section.mergeQueue'), [['Threshold', '15'], ['Strategy', 'rebase']]],
						[$t('settings.config.section.sync'), [['Interval', '5 min'], ['Full sync', '24h']]]
					] as [section, items]}
						<div class="border rounded p-3">
							<h4 class="text-xs font-semibold text-primary mb-2">{section}</h4>
							<dl class="grid grid-cols-[120px_1fr] gap-x-2 gap-y-0.5">
								{#each items as [key, val]}
									<dt class="text-xs text-muted-foreground">{key}</dt>
									<dd class="text-xs text-foreground/90 mono">{val}</dd>
								{/each}
							</dl>
						</div>
					{/each}
				</div>
			</Card.Content>
		</Card.Root>
	</Tabs.Content>

	<!-- ========================= RELIABILITY ========================= -->
	<Tabs.Content value="reliability" class="mt-2">
		<div class="w-full">
			<Card.Root>
				<Card.Header>
					<Card.Title>{$t('settings.retry.title')}</Card.Title>
					<Card.Description class="text-xs">{$t('settings.retry.helper')}</Card.Description>
				</Card.Header>
				<Card.Content>
					{#if retrySettings}
						{#if retryMessage}
							{@render statusAlert(retryMessage, retryError, 'mb-3')}
						{/if}

						<form onsubmit={(e) => { e.preventDefault(); handleSaveRetry(); }} class="space-y-4 max-w-md">
							<div class="flex items-center justify-between">
								<Label for="retry-enabled" class="text-xs">{$t('settings.retry.enabled')}</Label>
								<Switch id="retry-enabled" bind:checked={retrySettings.enabled} disabled={savingRetry} />
							</div>

							<div>
								<Label for="retry-attempts" class="text-xs mb-1">{$t('settings.retry.maxAttempts')}</Label>
								<Input id="retry-attempts" type="number" min="1" max="10" bind:value={retrySettings.max_attempts} disabled={savingRetry || !retrySettings.enabled} class="h-8" />
								<p class="text-xs text-muted-foreground mt-1">{$t('settings.retry.maxAttemptsHelp')}</p>
							</div>

							<div>
								<Label for="retry-initial" class="text-xs mb-1">{$t('settings.retry.initialBackoff')}</Label>
								<Input id="retry-initial" type="number" min="50" max="60000" step="50" bind:value={retrySettings.initial_backoff_ms} disabled={savingRetry || !retrySettings.enabled} class="h-8" />
								<p class="text-xs text-muted-foreground mt-1">{$t('settings.retry.initialBackoffHelp')}</p>
							</div>

							<div>
								<Label for="retry-max" class="text-xs mb-1">{$t('settings.retry.maxBackoff')}</Label>
								<Input id="retry-max" type="number" min="50" max="120000" step="100" bind:value={retrySettings.max_backoff_ms} disabled={savingRetry || !retrySettings.enabled} class="h-8" />
								<p class="text-xs text-muted-foreground mt-1">{$t('settings.retry.maxBackoffHelp')}</p>
							</div>

							<Button type="submit" disabled={savingRetry} size="sm" class="w-full">
								{savingRetry ? $t('common.saving') : $t('settings.retry.save')}
							</Button>
						</form>
					{:else}
						<p class="text-sm text-muted-foreground">{$t('common.loading')}</p>
					{/if}
				</Card.Content>
			</Card.Root>
		</div>
	</Tabs.Content>

	<!-- ========================= SECRETS ============================ -->
	<Tabs.Content value="secrets" class="mt-2">
		<!-- Disambiguation banner: this tab is for advanced / per-repo
		     secrets. Common GitHub / Anthropic tokens belong in their
		     dedicated tabs. -->
		<Alert.Root class="mb-4 border-primary/30 bg-primary/10 text-primary">
			<Alert.Description class="text-sm text-primary/90">
				<span class="font-semibold text-primary">{$t('settings.secrets.banner.title')}</span>
				{$t('settings.secrets.banner.body')}
			</Alert.Description>
		</Alert.Root>

		<!-- Doc / how-to: create a github_token. Toggleable so admins
		     who already know the drill don't see it every visit. -->
		<details class="mb-4 rounded border bg-card/60 open:bg-card">
			<summary class="cursor-pointer px-4 py-3 text-sm font-semibold text-primary hover:text-primary/80">
				ℹ️ {translate('secrets.help.title')}
			</summary>
			<div class="px-4 pb-4 pt-1 text-sm text-foreground/90 space-y-2">
				<p>{translate('secrets.help.intro')}</p>
				<ol class="list-decimal list-inside space-y-1 ms-2">
					<li>{translate('secrets.help.step1')}</li>
					<li>{translate('secrets.help.step2')}</li>
					<li>{translate('secrets.help.step3')}</li>
					<li>{translate('secrets.help.step4')}</li>
				</ol>
				<p class="text-xs text-muted-foreground italic">
					💡 {translate('secrets.help.tip')}
				</p>
				<a
					href="https://github.com/settings/tokens"
					target="_blank"
					rel="noopener noreferrer"
					class="inline-block mt-1 text-primary hover:text-primary/80 underline text-xs"
				>
					→ {translate('secrets.help.link')}
				</a>
			</div>
		</details>

		<div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
			<!-- Stored secrets list -->
			<Card.Root>
				<Card.Header>
					<Card.Title>{$t('settings.secrets.stored')}</Card.Title>
					<Card.Description class="text-xs">
						{$t('settings.secrets.encrypted')}
					</Card.Description>
				</Card.Header>
				<Card.Content>
					{#if secretsError}
						{@render statusAlert(secretsError, true, 'mb-2')}
					{/if}
					{#if secrets.length === 0}
						<p class="text-sm text-muted-foreground">{$t('settings.secrets.none')}</p>
					{:else}
						<Table.Root class="text-xs">
							<Table.Header>
								<Table.Row>
									<Table.Head>{$t('settings.secrets.col.scope')}</Table.Head>
									<Table.Head>{$t('settings.secrets.col.key')}</Table.Head>
									<Table.Head>{$t('settings.secrets.col.value')}</Table.Head>
									<Table.Head>{$t('settings.secrets.col.updated')}</Table.Head>
									<Table.Head><span class="sr-only">{$t('settings.secrets.actions')}</span></Table.Head>
								</Table.Row>
							</Table.Header>
							<Table.Body>
								{#each secrets as s (s.id)}
									<Table.Row>
										<Table.Cell>
											<Badge
												variant="outline"
												class={s.scope === 'global' ? 'bg-primary/15 text-primary' : 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400'}
											>
												{s.scope}{s.slug ? `: ${s.slug}` : ''}
											</Badge>
										</Table.Cell>
										<Table.Cell class="mono text-foreground">{s.key}</Table.Cell>
										<Table.Cell class="mono text-foreground/90">
											{revealedId === s.id && revealedValue ? revealedValue : '••••••••'}
										</Table.Cell>
										<Table.Cell class="text-muted-foreground">
											{new Date(s.updated_at).toLocaleString()}
										</Table.Cell>
										<Table.Cell class="text-right whitespace-nowrap">
											<Button variant="outline" size="xs" onclick={() => handleReveal(s.id)}>
												{revealedId === s.id ? $t('settings.secrets.hide') : $t('settings.secrets.reveal')}
											</Button>
											<Button variant="destructive" size="xs" onclick={() => handleDeleteSecret(s.id)}>
												{$t('common.delete')}
											</Button>
										</Table.Cell>
									</Table.Row>
								{/each}
							</Table.Body>
						</Table.Root>
					{/if}
				</Card.Content>
			</Card.Root>

			<!-- Add new secret -->
			<Card.Root>
				<Card.Header>
					<Card.Title>{$t('settings.secrets.add')}</Card.Title>
				</Card.Header>
				<Card.Content>
					{#if secretMessage}
						{@render statusAlert(secretMessage, secretMessageErr, 'mb-2')}
					{/if}
					<form onsubmit={(e) => { e.preventDefault(); handleAddSecret(); }} class="space-y-3">
						<div>
							<Label class="text-xs mb-1">{$t('settings.secrets.scope')}</Label>
							<RadioGroup.Root
								value={newSecretScope}
								onValueChange={(v) => (newSecretScope = v as 'global' | 'repo')}
								class="flex gap-3"
							>
								<div class="flex items-center gap-2">
									<RadioGroup.Item value="global" id="sec-scope-global" />
									<Label for="sec-scope-global" class="text-sm font-normal">{$t('settings.secrets.scope.global')}</Label>
								</div>
								<div class="flex items-center gap-2">
									<RadioGroup.Item value="repo" id="sec-scope-repo" />
									<Label for="sec-scope-repo" class="text-sm font-normal">{$t('settings.secrets.scope.repo')}</Label>
								</div>
							</RadioGroup.Root>
						</div>
						{#if newSecretScope === 'repo'}
							<div>
								<Label for="sec-slug" class="text-xs mb-1">{$t('settings.secrets.repoSlug')}</Label>
								<Input id="sec-slug" type="text" bind:value={newSecretSlug}
									placeholder="owner/repo" disabled={savingSecret} class="h-8" />
							</div>
						{/if}
						<div>
							<Label for="sec-key" class="text-xs mb-1">{$t('settings.secrets.key')}</Label>
							<Input id="sec-key" type="text" bind:value={newSecretKey}
								placeholder="github_token, anthropic_api_key, …"
								disabled={savingSecret} class="h-8" />
							<p class="text-xs text-muted-foreground mt-1">
								{$t('settings.secrets.commonKeys')} <code>github_token</code>, <code>anthropic_oauth_token</code>,
								<code>anthropic_api_key</code>.
							</p>
						</div>
						<div>
							<Label for="sec-value" class="text-xs mb-1">{$t('settings.secrets.value')}</Label>
							<Input id="sec-value" type="password" bind:value={newSecretValue}
								placeholder="paste secret value" disabled={savingSecret} class="h-8" />
						</div>
						<Button type="submit" size="sm" class="w-full"
							disabled={savingSecret || !newSecretKey.trim() || !newSecretValue
								|| (newSecretScope === 'repo' && !newSecretSlug.trim())}>
							{savingSecret ? $t('common.saving') : $t('settings.secrets.save')}
						</Button>
					</form>
				</Card.Content>
			</Card.Root>
		</div>
	</Tabs.Content>

	<!-- ========================= USERS (RBAC) ========================= -->
	<Tabs.Content value="users" class="mt-2">
		<Card.Root>
			<Card.Header>
				<Card.Title>{$t('settings.users.title')}</Card.Title>
				<Card.Description class="text-xs">
					{$t('settings.users.helper')}
				</Card.Description>
				<Card.Action>
					<Button size="sm" onclick={openCreateUser} class="shrink-0">
						{$t('settings.users.addUser')}
					</Button>
				</Card.Action>
			</Card.Header>
			<Card.Content>
				{#if usersError}
					{@render statusAlert(usersError, true, 'mb-2')}
				{/if}
				{#if userMessage}
					{@render statusAlert(userMessage, userMessageErr, 'mb-2')}
				{/if}
				{#if users.length === 0}
					<p class="text-sm text-muted-foreground">{$t('settings.users.none')}</p>
				{:else}
					<Table.Root class="text-xs">
						<Table.Header>
							<Table.Row>
								<Table.Head>{$t('settings.users.col.identity')}</Table.Head>
								<Table.Head>{$t('settings.users.col.auth')}</Table.Head>
								<Table.Head>{$t('settings.users.col.role')}</Table.Head>
								<Table.Head>{$t('settings.users.col.lastLogin')}</Table.Head>
								<Table.Head><span class="sr-only">{$t('settings.secrets.actions')}</span></Table.Head>
							</Table.Row>
						</Table.Header>
						<Table.Body>
							{#each users as u (u.id)}
								<Table.Row>
									<Table.Cell>
										<div class="mono text-foreground">{u.email}</div>
										{#if u.username && u.username !== u.email}
											<div class="text-[0.65rem] text-muted-foreground">@{u.username}</div>
										{/if}
									</Table.Cell>
									<Table.Cell>
										<Badge
											variant="outline"
											class={u.sso_provider
												? 'border-purple-500/30 bg-purple-500/15 text-purple-600 dark:text-purple-400'
												: 'bg-primary/15 text-primary'}
										>
											{u.sso_provider ?? 'local'}
										</Badge>
									</Table.Cell>
									<Table.Cell>
										<Badge
											variant={u.role === 'admin' || u.role === 'operator' || u.role === 'member' ? 'outline' : 'secondary'}
											class={u.role === 'admin'
												? 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400'
												: u.role === 'operator'
													? 'border-orange-500/30 bg-orange-500/15 text-orange-600 dark:text-orange-400'
													: u.role === 'member'
														? 'bg-primary/15 text-primary'
														: ''}
										>
											{u.role}
										</Badge>
									</Table.Cell>
									<Table.Cell class="text-muted-foreground">
										{u.last_login_at ? new Date(u.last_login_at).toLocaleString() : '—'}
									</Table.Cell>
									<Table.Cell class="text-right whitespace-nowrap">
										<Button variant="outline" size="xs" onclick={() => openEditUser(u)}>
											{$t('common.edit')}
										</Button>
										<Button variant="destructive" size="xs" onclick={() => openDeleteUser(u)}>
											{$t('common.delete')}
										</Button>
									</Table.Cell>
								</Table.Row>
							{/each}
						</Table.Body>
					</Table.Root>
				{/if}
			</Card.Content>
		</Card.Root>
	</Tabs.Content>
</Tabs.Root>

<!-- Create user modal -->
<Dialog.Root bind:open={createUserModalOpen}>
	<Dialog.Content class="sm:max-w-lg">
		<Dialog.Header>
			<Dialog.Title>{$t('settings.users.modal.create.title')}</Dialog.Title>
		</Dialog.Header>
		<form onsubmit={(e) => { e.preventDefault(); handleCreateUser(); }} class="space-y-3">
			<div>
				<Label for="user-email" class="text-xs mb-1">{$t('settings.users.email')}</Label>
				<Input id="user-email" type="text" bind:value={newUserEmail}
					placeholder="alice@example.com or alice" disabled={creatingUser} class="h-8" />
			</div>
			<div>
				<Label for="user-username" class="text-xs mb-1">{$t('settings.users.username')}</Label>
				<Input id="user-username" type="text" bind:value={newUserUsername}
					placeholder="alice" disabled={creatingUser} class="h-8" />
			</div>
			<div>
				<Label for="user-password" class="text-xs mb-1">{$t('settings.users.password')}</Label>
				<Input id="user-password" type="password" bind:value={newUserPassword}
					placeholder={$t('settings.users.password.placeholder')} disabled={creatingUser} class="h-8" />
			</div>
			<div>
				<Label class="text-xs mb-1">{$t('settings.users.role')}</Label>
				<RadioGroup.Root
					value={newUserRole}
					onValueChange={(v) => (newUserRole = v as Role)}
					class="flex flex-col gap-1"
				>
					<div class="flex items-center gap-2">
						<RadioGroup.Item value="admin" id="new-role-admin" />
						<Label for="new-role-admin" class="text-sm font-normal">
							<span class="font-semibold">{$t('settings.users.role.admin')}</span>
							<span class="text-xs text-muted-foreground">{$t('settings.users.role.admin.help')}</span>
						</Label>
					</div>
					<div class="flex items-center gap-2">
						<RadioGroup.Item value="operator" id="new-role-operator" />
						<Label for="new-role-operator" class="text-sm font-normal">
							<span class="font-semibold">{$t('settings.users.role.operator')}</span>
							<span class="text-xs text-muted-foreground">{$t('settings.users.role.operator.help')}</span>
						</Label>
					</div>
					<div class="flex items-center gap-2">
						<RadioGroup.Item value="member" id="new-role-member" />
						<Label for="new-role-member" class="text-sm font-normal">
							<span class="font-semibold">{$t('settings.users.role.member')}</span>
							<span class="text-xs text-muted-foreground">{$t('settings.users.role.member.help')}</span>
						</Label>
					</div>
					<div class="flex items-center gap-2">
						<RadioGroup.Item value="viewer" id="new-role-viewer" />
						<Label for="new-role-viewer" class="text-sm font-normal">
							<span class="font-semibold">{$t('settings.users.role.viewer')}</span>
							<span class="text-xs text-muted-foreground">{$t('settings.users.role.viewer.help')}</span>
						</Label>
					</div>
				</RadioGroup.Root>
			</div>
			<div class="flex gap-2 pt-2">
				<Button variant="outline" size="sm" class="flex-1"
					onclick={() => createUserModalOpen = false} disabled={creatingUser}>
					{$t('common.cancel')}
				</Button>
				<Button type="submit" size="sm" class="flex-1"
					disabled={creatingUser || !newUserEmail.trim() || !newUserPassword || newUserPassword.length < 6}>
					{creatingUser ? $t('settings.users.creating') : $t('settings.users.create')}
				</Button>
			</div>
		</form>
	</Dialog.Content>
</Dialog.Root>

<!-- Edit user modal -->
<Dialog.Root bind:open={editUserModalOpen}>
	<Dialog.Content class="sm:max-w-lg">
		<Dialog.Header>
			<Dialog.Title>
				{editingUser ? `${$t('settings.users.modal.edit.titlePrefix')} ${editingUser.email}` : $t('settings.users.modal.edit.titleFallback')}
			</Dialog.Title>
		</Dialog.Header>
		{#if editingUser}
			<form onsubmit={(e) => { e.preventDefault(); handleSaveEdit(); }} class="space-y-3">
				<div>
					<Label class="text-xs mb-1">{$t('settings.users.role')}</Label>
					<RadioGroup.Root
						value={editRole}
						onValueChange={(v) => (editRole = v as Role)}
						class="flex flex-col gap-1"
					>
						<div class="flex items-center gap-2">
							<RadioGroup.Item value="admin" id="edit-role-admin" />
							<Label for="edit-role-admin" class="text-sm font-normal">
								<span class="font-semibold">{$t('settings.users.role.admin')}</span>
								<span class="text-xs text-muted-foreground">{$t('settings.users.role.admin.help.short')}</span>
							</Label>
						</div>
						<div class="flex items-center gap-2">
							<RadioGroup.Item value="operator" id="edit-role-operator" />
							<Label for="edit-role-operator" class="text-sm font-normal">
								<span class="font-semibold">{$t('settings.users.role.operator')}</span>
								<span class="text-xs text-muted-foreground">{$t('settings.users.role.operator.help')}</span>
							</Label>
						</div>
						<div class="flex items-center gap-2">
							<RadioGroup.Item value="member" id="edit-role-member" />
							<Label for="edit-role-member" class="text-sm font-normal">
								<span class="font-semibold">{$t('settings.users.role.member')}</span>
								<span class="text-xs text-muted-foreground">{$t('settings.users.role.member.help')}</span>
							</Label>
						</div>
						<div class="flex items-center gap-2">
							<RadioGroup.Item value="viewer" id="edit-role-viewer" />
							<Label for="edit-role-viewer" class="text-sm font-normal">
								<span class="font-semibold">{$t('settings.users.role.viewer')}</span>
								<span class="text-xs text-muted-foreground">{$t('settings.users.role.viewer.help')}</span>
							</Label>
						</div>
					</RadioGroup.Root>
				</div>
				<div>
					<Label for="edit-pw" class="text-xs mb-1">{$t('settings.users.newPassword')}</Label>
					<Input id="edit-pw" type="password" bind:value={editPassword}
						placeholder={$t('settings.users.password.placeholder')} disabled={savingEdit} class="h-8" />
				</div>
				<div class="flex gap-2 pt-2">
					<Button variant="outline" size="sm" class="flex-1"
						onclick={() => editUserModalOpen = false} disabled={savingEdit}>
						{$t('common.cancel')}
					</Button>
					<Button type="submit" size="sm" class="flex-1" disabled={savingEdit}>
						{savingEdit ? $t('common.saving') : $t('common.save')}
					</Button>
				</div>
			</form>
		{/if}
	</Dialog.Content>
</Dialog.Root>

<!-- Edit features modal -->
<Dialog.Root bind:open={featuresModalOpen}>
	<Dialog.Content class="max-h-[85vh] overflow-y-auto sm:max-w-2xl">
		<Dialog.Header>
			<Dialog.Title>
				{featuresSlug ? `${$t('settings.features.modalTitleFor')} ${featuresSlug}` : $t('settings.features.modalTitle')}
			</Dialog.Title>
		</Dialog.Header>
		{#if featuresMessage}
			{@render statusAlert(featuresMessage, featuresMessageErr, 'mb-3')}
		{/if}
		{#if !featuresDraft}
			<p class="text-sm text-muted-foreground">{$t('common.loading')}</p>
		{:else}
			<div class="space-y-4">
				<!-- Master mode: dry-run vs apply. Switches all write-back actions. -->
				<div
					class="rounded-lg border p-3 transition-colors {featuresDraft.apply
						? 'border-green-500/30 bg-green-500/10'
						: 'border-yellow-500/40 bg-yellow-500/10'}"
				>
					<div class="flex items-center justify-between gap-3">
						<div>
							<h4 class="text-sm font-semibold {featuresDraft.apply ? 'text-green-700 dark:text-green-300' : 'text-yellow-700 dark:text-yellow-300'} flex items-center gap-1">
								{featuresDraft.apply
									? $t('settings.features.mode.apply')
									: $t('settings.features.mode.dryrun')}
								{@render infoTip('mode-tip', 'settings.features.mode.tip')}
							</h4>
							<p class="text-xs text-muted-foreground mt-0.5">
								{featuresDraft.apply
									? $t('settings.features.mode.body.apply')
									: $t('settings.features.mode.body.dryrun')}
							</p>
						</div>
						<Switch bind:checked={featuresDraft.apply} class="shrink-0" />
					</div>
				</div>

				<div>
					<h4 class="text-xs uppercase text-muted-foreground font-semibold mb-2">{$t('settings.features.collection.title')}</h4>
					<p class="text-xs text-muted-foreground mb-2">
						{$t('settings.features.collection.body')}
					</p>
					<div class="space-y-1.5">
						<label class="flex items-center gap-2 text-sm">
							<input type="checkbox" bind:checked={featuresDraft.collect_issues} class="rounded" />
							<span><strong>{$t('settings.features.collection.issues')}</strong> <span class="text-xs text-muted-foreground">{$t('settings.features.collection.issues.help')}</span></span>
							{@render infoTip('tip-collect-issues', 'settings.features.collection.issues.tip')}
						</label>
						<label class="flex items-center gap-2 text-sm">
							<input type="checkbox" bind:checked={featuresDraft.collect_prs} class="rounded" />
							<span><strong>{$t('settings.features.collection.prs')}</strong> <span class="text-xs text-muted-foreground">{$t('settings.features.collection.prs.help')}</span></span>
							{@render infoTip('tip-collect-prs', 'settings.features.collection.prs.tip')}
						</label>
					</div>
				</div>

				<div class:opacity-60={!featuresDraft.apply}>
					<h4 class="text-xs uppercase text-muted-foreground font-semibold mb-2">
						{$t('settings.features.ai.title')}
						{#if !featuresDraft.apply}
							<span class="ml-2 text-yellow-600 dark:text-yellow-500/80 normal-case font-normal">{$t('settings.features.ai.dimmed')}</span>
						{/if}
					</h4>
					<p class="text-xs text-muted-foreground mb-2">
						{$t('settings.features.ai.body')}
					</p>
					<div class="space-y-1.5">
						<label class="flex items-center gap-2 text-sm">
							<input type="checkbox" bind:checked={featuresDraft.triage_issues} class="rounded" />
							<span><strong>{$t('settings.features.ai.triage')}</strong> <span class="text-xs text-muted-foreground">{$t('settings.features.ai.triage.help')}</span></span>
							{@render infoTip('tip-triage', 'settings.features.ai.triage.tip')}
						</label>
						<label class="flex items-center gap-2 text-sm">
							<input type="checkbox" bind:checked={featuresDraft.analyze_prs} class="rounded" />
							<span><strong>{$t('settings.features.ai.analyze')}</strong> <span class="text-xs text-muted-foreground">{$t('settings.features.ai.analyze.help')}</span></span>
							{@render infoTip('tip-analyze', 'settings.features.ai.analyze.tip')}
						</label>
						<label class="flex items-center gap-2 text-sm">
							<input type="checkbox" bind:checked={featuresDraft.review_prs} class="rounded" />
							<span>
								<strong>{$t('settings.features.ai.review')}</strong>
								<span class="text-xs text-muted-foreground">{$t('settings.features.ai.review.help')}</span>
							</span>
							{@render infoTip('tip-review', 'settings.features.ai.review.tip')}
						</label>
					</div>
				</div>

				<div class:opacity-60={!featuresDraft.apply}>
					<h4 class="text-xs uppercase text-muted-foreground font-semibold mb-2">
						{$t('settings.features.auto.title')}
						{#if !featuresDraft.apply}
							<span class="ml-2 text-yellow-600 dark:text-yellow-500/80 normal-case font-normal">{$t('settings.features.auto.dimmed')}</span>
						{/if}
					</h4>
					<p class="text-xs text-muted-foreground mb-2">
						{$t('settings.features.auto.body')}
					</p>
					<div class="space-y-1.5">
						<label class="flex items-center gap-2 text-sm">
							<input type="checkbox" bind:checked={featuresDraft.auto_pr} class="rounded" />
							<span><strong>{$t('settings.features.auto.fix')}</strong> <span class="text-xs text-muted-foreground">{$t('settings.features.auto.fix.help')}</span></span>
							{@render infoTip('tip-autopr', 'settings.features.auto.fix.tip')}
						</label>
						<label class="flex items-center gap-2 text-sm">
							<input type="checkbox" bind:checked={featuresDraft.auto_merge} class="rounded" />
							<span><strong>{$t('settings.features.auto.merge')}</strong> <span class="text-xs text-muted-foreground">{$t('settings.features.auto.merge.help')}</span></span>
							{@render infoTip('tip-automerge', 'settings.features.auto.merge.tip')}
						</label>
					</div>
				</div>

				<!-- Advanced filters: collapsible. Free-text comma-separated for arrays. -->
				<details class="rounded border bg-muted/40">
					<summary class="cursor-pointer px-3 py-2 text-sm font-semibold text-primary hover:text-primary/80">
						{$t('settings.advancedFilters')}
					</summary>
					<div class="p-3 space-y-3 text-sm">
						<!-- One-click defaults aligned with GitHub's standard label set. -->
						<div class="flex items-start justify-between gap-3 rounded border border-primary/30 bg-primary/10 p-3">
							<div class="text-xs">
								<p class="font-semibold text-primary mb-1">{$t('settings.advancedFilters.defaults.title')}</p>
								<p class="text-muted-foreground">
									{$t('settings.advancedFilters.defaults.body')}
								</p>
							</div>
							<Button size="xs" onclick={applyGithubDefaults} class="shrink-0">
								{$t('settings.advancedFilters.defaults.apply')}
							</Button>
						</div>

						<details class="rounded border bg-muted/40">
							<summary class="cursor-pointer px-3 py-2 text-xs font-semibold text-muted-foreground hover:text-foreground">
								{$t('settings.advancedFilters.defaults.help')}
							</summary>
							<div class="p-3 text-xs space-y-1 text-muted-foreground">
								<div><code class="text-red-600 dark:text-red-300">bug</code> — Something isn't working. <em>Triage candidate.</em></div>
								<div><code class="text-cyan-600 dark:text-cyan-300">enhancement</code> — New feature or request.</div>
								<div><code class="text-primary">documentation</code> — Doc improvements.</div>
								<div><code class="text-yellow-600 dark:text-yellow-300">good first issue</code> — Good for newcomers. <em>Auto-fix candidate.</em></div>
								<div><code class="text-green-600 dark:text-green-300">help wanted</code> — Extra attention is needed.</div>
								<div><code class="text-purple-600 dark:text-purple-300">question</code> — Further info requested. <em>Skip triage (human judgment).</em></div>
								<div><code class="text-muted-foreground">duplicate</code> — Already exists. <em>Skip triage.</em></div>
								<div><code class="text-muted-foreground">invalid</code> — Doesn't seem right. <em>Skip triage.</em></div>
								<div><code class="text-muted-foreground">wontfix</code> — Will not be worked on. <em>Skip triage.</em></div>
							</div>
						</details>
						<div>
							<h5 class="text-xs uppercase text-muted-foreground font-semibold mb-1">{$t('settings.advancedFilters.section.global')}</h5>
							<Label class="text-xs mb-1">{$t('settings.advancedFilters.skipAuthors')}</Label>
							<Input
								class="h-8"
								placeholder="dependabot[bot], renovate[bot]"
								value={featuresDraft.filters.skip_authors.join(', ')}
								onchange={(e) => {
									featuresDraft!.filters.skip_authors = parseCsv((e.currentTarget as HTMLInputElement).value);
								}}
							/>
							<Label class="text-xs mb-1 mt-2">{$t('settings.advancedFilters.targetBranches')}</Label>
							<Input
								class="h-8"
								placeholder="main, develop"
								value={featuresDraft.filters.target_branches.join(', ')}
								onchange={(e) => {
									featuresDraft!.filters.target_branches = parseCsv((e.currentTarget as HTMLInputElement).value);
								}}
							/>
							<label class="flex items-center gap-2 text-sm mt-2">
								<input type="checkbox" bind:checked={featuresDraft.filters.skip_drafts} class="rounded" />
								<span>{$t('settings.advancedFilters.skipDrafts')}</span>
							</label>
						</div>

						<div>
							<h5 class="text-xs uppercase text-muted-foreground font-semibold mb-1">{$t('settings.advancedFilters.section.triage')}</h5>
							<Label class="text-xs mb-1">{$t('settings.advancedFilters.onlyLabels')}</Label>
							<Input
								class="h-8"
								placeholder="needs-triage, bug"
								value={featuresDraft.filters.triage_only_labels.join(', ')}
								onchange={(e) => {
									featuresDraft!.filters.triage_only_labels = parseCsv((e.currentTarget as HTMLInputElement).value);
								}}
							/>
							<Label class="text-xs mb-1 mt-2">{$t('settings.advancedFilters.skipLabels')}</Label>
							<Input
								class="h-8"
								placeholder="wontfix, duplicate"
								value={featuresDraft.filters.triage_skip_labels.join(', ')}
								onchange={(e) => {
									featuresDraft!.filters.triage_skip_labels = parseCsv((e.currentTarget as HTMLInputElement).value);
								}}
							/>
							<Label class="text-xs mb-1 mt-2">{$t('settings.advancedFilters.maxAge')}</Label>
							<Input
								type="number"
								class="h-8"
								bind:value={featuresDraft.filters.triage_max_age_days}
							/>
						</div>

						<div>
							<h5 class="text-xs uppercase text-muted-foreground font-semibold mb-1">{$t('settings.advancedFilters.section.analyze')}</h5>
							<div class="grid grid-cols-2 gap-2">
								<div>
									<Label class="text-xs mb-1">{$t('settings.advancedFilters.minLoc')}</Label>
									<Input type="number" class="h-8" bind:value={featuresDraft.filters.analyze_min_loc} />
								</div>
								<div>
									<Label class="text-xs mb-1">{$t('settings.advancedFilters.maxLoc')}</Label>
									<Input type="number" class="h-8" bind:value={featuresDraft.filters.analyze_max_loc} />
								</div>
							</div>
						</div>

						<div>
							<h5 class="text-xs uppercase text-muted-foreground font-semibold mb-1">{$t('settings.advancedFilters.section.autoFix')}</h5>
							<Label class="text-xs mb-1">{$t('settings.advancedFilters.onlyLabels')}</Label>
							<Input
								class="h-8"
								placeholder="good-first-issue, auto-fix"
								value={featuresDraft.filters.auto_pr_only_labels.join(', ')}
								onchange={(e) => {
									featuresDraft!.filters.auto_pr_only_labels = parseCsv((e.currentTarget as HTMLInputElement).value);
								}}
							/>
							<Label class="text-xs mb-1 mt-2">{$t('settings.advancedFilters.targetBranch')}</Label>
							<Input class="h-8" placeholder="main" bind:value={featuresDraft.filters.auto_pr_target_branch} />
						</div>

						<div>
							<h5 class="text-xs uppercase text-muted-foreground font-semibold mb-1">{$t('settings.advancedFilters.section.autoMerge')}</h5>
							<Label class="text-xs mb-1">{$t('settings.advancedFilters.onlyAuthors')}</Label>
							<Input
								class="h-8"
								placeholder="dependabot[bot]"
								value={featuresDraft.filters.auto_merge_only_authors.join(', ')}
								onchange={(e) => {
									featuresDraft!.filters.auto_merge_only_authors = parseCsv((e.currentTarget as HTMLInputElement).value);
								}}
							/>
							<Label class="text-xs mb-1 mt-2">{$t('settings.advancedFilters.onlyLabels')}</Label>
							<Input
								class="h-8"
								placeholder="auto-merge"
								value={featuresDraft.filters.auto_merge_only_labels.join(', ')}
								onchange={(e) => {
									featuresDraft!.filters.auto_merge_only_labels = parseCsv((e.currentTarget as HTMLInputElement).value);
								}}
							/>
							<div class="grid grid-cols-2 gap-2 mt-2">
								<div>
									<Label class="text-xs mb-1">{$t('settings.advancedFilters.minApprovals')}</Label>
									<Input type="number" class="h-8" bind:value={featuresDraft.filters.auto_merge_min_approvals} />
								</div>
								<div>
									<Label class="text-xs mb-1">{$t('settings.advancedFilters.maxLoc')}</Label>
									<Input type="number" class="h-8" bind:value={featuresDraft.filters.auto_merge_max_loc} />
								</div>
							</div>
						</div>
					</div>
				</details>

				<div class="flex gap-2 pt-2">
					<Button variant="outline" size="sm" class="flex-1"
						onclick={() => featuresModalOpen = false} disabled={featuresSaving}>
						{$t('common.cancel')}
					</Button>
					<Button size="sm" class="flex-1"
						onclick={handleSaveFeatures} disabled={featuresSaving}>
						{featuresSaving ? $t('common.saving') : $t('common.save')}
					</Button>
				</div>
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>

<!-- Delete user confirm modal -->
<Dialog.Root bind:open={deleteUserModalOpen}>
	<Dialog.Content class="sm:max-w-md">
		<Dialog.Header>
			<Dialog.Title>{$t('settings.users.modal.delete.title')}</Dialog.Title>
		</Dialog.Header>
		{#if deletingUser}
			<p class="text-sm">
				{$t('settings.users.modal.delete.prefix')} <span class="mono text-destructive">{deletingUser.email}</span>{$t('settings.users.modal.delete.confirm')}
			</p>
			<div class="flex gap-2 pt-4">
				<Button variant="outline" size="sm" class="flex-1"
					onclick={() => deleteUserModalOpen = false} disabled={deletingNow}>
					{$t('common.cancel')}
				</Button>
				<Button variant="destructive" size="sm" class="flex-1"
					onclick={handleConfirmDelete} disabled={deletingNow}>
					{deletingNow ? $t('settings.users.deleting') : $t('common.delete')}
				</Button>
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>
