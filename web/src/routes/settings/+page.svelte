<script lang="ts">
	import { onMount } from 'svelte';
	import {
		Card,
		Badge,
		Tabs,
		TabItem,
		Button,
		Input,
		Label,
		Helper,
		Alert,
		Heading,
		Radio,
		Table,
		TableHead,
		TableHeadCell,
		TableBody,
		TableBodyRow,
		TableBodyCell,
		Modal,
		Tooltip,
		Toggle,
	} from 'flowbite-svelte';
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
		fetchRetrySettings,
		updateRetrySettings,
		type RepoFeatures,
		type RetrySettings,
		type LicenseInfo,
		type ReposListResponse,
		type AuthStatus,
		type SecretRecord,
		type UserRecord,
		type Role,
	} from '$lib/api';

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
	<button
		type="button"
		{id}
		aria-label={$t('common.moreInfo')}
		class="inline-flex items-center justify-center w-4 h-4 rounded-full bg-gray-700 text-gray-300 text-[10px] font-bold hover:bg-blue-600 hover:text-white transition-colors cursor-help"
	>?</button>
	<Tooltip triggeredBy="#{id}" placement="right" class="max-w-xs text-xs leading-snug">
		{$t(bodyKey)}
	</Tooltip>
{/snippet}

<svelte:head>
	<title>wshm - Settings</title>
</svelte:head>

<div class="mb-4">
	<Heading tag="h2" class="text-xl mb-1">{$t('settings.title')}</Heading>
	<p class="text-sm text-gray-500">{$t('settings.subtitle')}</p>
</div>

<Tabs tabStyle="underline" contentClass="bg-transparent p-0 mt-4">
	<!-- ========================= REPOSITORIES ========================= -->
	<TabItem open title={$t('settings.tabs.repos')}>
		<div class="w-full">
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-4">{$t('settings.repos.title')}</Heading>

				{#if reposList}
					<div class="mb-3">
						<h4 class="text-xs font-semibold text-blue-400 mb-2">{$t('settings.repos.configured')} ({reposList.repos.length})</h4>
						{#if reposList.repos.length === 0}
							<p class="text-xs text-gray-500">{$t('settings.repos.none')}</p>
						{:else}
							<ul class="space-y-1 text-xs">
								{#each reposList.repos as r}
									<li class="flex items-center justify-between gap-2">
										<span class="text-gray-300 mono">{r.slug}</span>
										<div class="flex items-center gap-2">
											<Badge color={r.apply ? 'green' : 'dark'}>{r.apply ? $t('settings.repos.badge.apply') : $t('settings.repos.badge.dryrun')}</Badge>
											<Button color="alternative" size="xs" onclick={() => openFeaturesModal(r.slug)}>
												{$t('settings.repos.editFeatures')}
											</Button>
										</div>
									</li>
								{/each}
							</ul>
						{/if}
					</div>

					<div class="border-t border-gray-700 pt-3 space-y-2">
						{#if addRepoMessage}
							<Alert color={addRepoError ? 'red' : 'green'} class="text-xs py-2">{addRepoMessage}</Alert>
						{/if}

						{#if reposList.dynamic_add_supported}
							<form onsubmit={(e) => { e.preventDefault(); handleAddRepo(); }} class="space-y-2">
								<div>
									<Label for="repo-slug" class="text-xs mb-1">{$t('settings.repos.slug')}</Label>
									<Input id="repo-slug" type="text" bind:value={newRepoSlug} placeholder="owner/repo" disabled={addingRepo} size="sm" />
								</div>
								<div>
									<Label for="repo-path" class="text-xs mb-1">{$t('settings.repos.pathOptional')}</Label>
									<Input id="repo-path" type="text" bind:value={newRepoPath} placeholder="/abs/path" disabled={addingRepo} size="sm" />
								</div>
								<Button type="submit" color="blue" disabled={addingRepo || !newRepoSlug.trim()} size="sm" class="w-full">
									{addingRepo ? $t('settings.repos.adding') : $t('settings.repos.add')}
								</Button>
							</form>
						{:else}
							<Helper>
								{$t('settings.repos.dynamicNotAvailable')}
								<code class="rounded bg-gray-700 px-1 py-0.5">~/.wshm/global.toml</code>
								{$t('settings.repos.dynamicNotAvailable.suffix')}
							</Helper>
						{/if}
					</div>
				{:else}
					<p class="text-sm text-gray-500">{$t('common.loading')}</p>
				{/if}
			</Card>
		</div>
	</TabItem>

	<!-- ========================= GIT PROVIDERS ========================= -->
	<TabItem title={$t('settings.tabs.gitProviders')}>
		<div class="w-full">
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-4">{$t('settings.git.title')}</Heading>

				{#if authStatus}
					<div class="mb-3">
						<Badge large color={authStatus.github ? 'green' : 'dark'}>
							{authStatus.github ? $t('settings.git.configured') : $t('settings.git.notConfigured')}
						</Badge>
					</div>

					<Helper class="mb-3">
						{$t('settings.git.helper.intro')} <a href="https://github.com/settings/tokens" target="_blank" class="text-blue-400 hover:underline">{$t('settings.git.helper.generate')}</a> {$t('settings.git.helper.scope')}
					</Helper>

					{#if ghMessage}
						<Alert color={ghError ? 'red' : 'green'} class="text-xs py-2 mb-2">{ghMessage}</Alert>
					{/if}

					<form onsubmit={(e) => { e.preventDefault(); handleSetGithub(); }} class="space-y-2">
						<div>
							<Label for="gh-token" class="text-xs mb-1">{$t('settings.git.token')}</Label>
							<Input id="gh-token" type="password" bind:value={ghToken} placeholder="ghp_..." disabled={savingGh} size="sm" />
						</div>
						<Button type="submit" color="blue" disabled={savingGh || !ghToken.trim()} size="sm" class="w-full">
							{savingGh ? $t('common.saving') : $t('settings.git.save')}
						</Button>
						{#if authStatus.github}
							<Button type="button" color="red" outline disabled={savingGh} size="sm" class="w-full" onclick={handleRemoveGithub}>
								{$t('settings.git.remove')}
							</Button>
						{/if}
					</form>
				{:else}
					<p class="text-sm text-gray-500">{$t('common.loading')}</p>
				{/if}
			</Card>
			<Helper class="mt-3 text-xs">{$t('settings.git.moreSoon')}</Helper>
		</div>
	</TabItem>

	<!-- ========================= AI PROVIDERS ========================= -->
	<TabItem title={$t('settings.tabs.aiProviders')}>
		<div class="w-full">
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-4">{$t('settings.ai.title')}</Heading>

				{#if authStatus}
					<div class="mb-3">
						<Badge large color={authStatus.anthropic ? 'green' : 'dark'}>
							{authStatus.anthropic === 'oauth'
								? $t('settings.ai.badge.oauth')
								: authStatus.anthropic === 'api_key'
									? $t('settings.ai.badge.apiKey')
									: $t('settings.ai.badge.notConfigured')}
						</Badge>
					</div>

					<Helper class="mb-3">
						{$t('settings.ai.helper')} <code class="rounded bg-gray-700 px-1 py-0.5">claude /token</code> {$t('settings.ai.helper.suffix')}
						<a href="https://console.anthropic.com/" target="_blank" class="text-blue-400 hover:underline">{$t('settings.ai.helper.apiKey')}</a>.
					</Helper>

					{#if anthropicMessage}
						<Alert color={anthropicError ? 'red' : 'green'} class="text-xs py-2 mb-2">{anthropicMessage}</Alert>
					{/if}

					<form onsubmit={(e) => { e.preventDefault(); handleSetAnthropic(); }} class="space-y-2">
						<div class="flex gap-4 text-xs">
							<Radio bind:group={anthropicKind} value="oauth" disabled={savingAnthropic}>{$t('settings.ai.kind.oauth')}</Radio>
							<Radio bind:group={anthropicKind} value="api_key" disabled={savingAnthropic}>{$t('settings.ai.kind.apiKey')}</Radio>
						</div>
						<div>
							<Label for="anth-token" class="text-xs mb-1">{$t('settings.git.token')}</Label>
							<Input id="anth-token" type="password" bind:value={anthropicToken} placeholder={anthropicKind === 'oauth' ? 'sk-ant-oat01-...' : 'sk-ant-api03-...'} disabled={savingAnthropic} size="sm" />
						</div>
						<Button type="submit" color="blue" disabled={savingAnthropic || !anthropicToken.trim()} size="sm" class="w-full">
							{savingAnthropic ? $t('common.saving') : $t('settings.ai.save')}
						</Button>
						{#if authStatus.anthropic}
							<Button type="button" color="red" outline disabled={savingAnthropic} size="sm" class="w-full" onclick={handleRemoveAnthropic}>
								{$t('settings.ai.remove')}
							</Button>
						{/if}
					</form>
				{:else}
					<p class="text-sm text-gray-500">{$t('common.loading')}</p>
				{/if}
			</Card>
			<Helper class="mt-3 text-xs">{$t('settings.ai.moreSoon')}</Helper>
		</div>
	</TabItem>

	<!-- ============================ LICENSE ============================ -->
	<TabItem title={$t('settings.tabs.license')}>
		<Card class="bg-gray-800 border-gray-700 max-w-none">
			<Heading tag="h3" class="text-base mb-4">{$t('settings.license.title')}</Heading>

			{#if license}
				<div class="flex items-center gap-3 mb-4">
					<Badge large color={license.is_pro ? 'green' : 'dark'}>{license.plan.toUpperCase()}</Badge>
					{#if !license.is_pro}
						<span class="text-sm text-gray-400">{$t('settings.license.free')}</span>
					{/if}
				</div>

				<div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-4">
					<div>
						<h4 class="text-sm font-semibold text-gray-400 mb-2">{$t('settings.license.ossFeatures')}</h4>
						<div class="flex flex-wrap gap-1">
							{#each license.oss_features as f}
								<Badge color="blue">{f}</Badge>
							{/each}
						</div>
					</div>
					<div>
						<h4 class="text-sm font-semibold text-gray-400 mb-2">{$t('settings.license.proFeatures')}</h4>
						<div class="space-y-1">
							{#each license.features as f}
								<div class="flex items-center justify-between text-sm">
									<span class="text-gray-300">{f.label}</span>
									<Badge color={f.enabled ? 'green' : 'dark'}>{f.enabled ? $t('settings.license.feature.active') : $t('settings.license.feature.locked')}</Badge>
								</div>
							{/each}
						</div>
					</div>
				</div>

				<div class="border-t border-gray-700 pt-3">
					{#if activateMessage}
						<Alert color={activateError ? 'red' : 'green'} class="text-xs py-2 mb-2">{activateMessage}</Alert>
					{/if}

					<Helper class="mb-2">{license.is_pro ? $t('settings.license.update') : $t('settings.license.enter')}</Helper>
					<form onsubmit={(e) => { e.preventDefault(); handleActivate(); }} class="flex gap-2">
						<Input type="text" bind:value={licenseKey} placeholder="wshm-pro-xxxx-xxxx-xxxx" disabled={activating} size="sm" class="flex-1" />
						<Button type="submit" color="blue" disabled={activating || !licenseKey.trim()} size="sm">
							{activating ? $t('settings.license.activating') : $t('settings.license.activate')}
						</Button>
					</form>

					{#if !license.is_pro}
						<p class="text-xs text-gray-500 mt-2">
							<a href="https://wshm.dev/pro" target="_blank" class="text-blue-400 hover:underline">{$t('settings.license.getLicense')}</a>
						</p>
					{/if}
				</div>
			{:else}
				<p class="text-sm text-gray-500">{$t('common.loading')}</p>
			{/if}
		</Card>
	</TabItem>

	<!-- ========================== APPEARANCE ========================== -->
	<TabItem title={$t('settings.tabs.appearance')}>
		<Card class="bg-gray-800 border-gray-700 max-w-none mb-4">
			<div class="grid grid-cols-1 sm:grid-cols-2 gap-6">
				<div>
					<Heading tag="h3" class="text-base mb-3">{$t('settings.appearance.theme')}</Heading>
					<div class="flex gap-2">
						<Button size="xs" color={$theme === 'dark' ? 'blue' : 'alternative'} onclick={() => theme.set('dark')}>
							{$t('settings.appearance.themeDark')}
						</Button>
						<Button size="xs" color={$theme === 'light' ? 'blue' : 'alternative'} onclick={() => theme.set('light')}>
							{$t('settings.appearance.themeLight')}
						</Button>
					</div>
				</div>
				<div>
					<Heading tag="h3" class="text-base mb-3">{$t('settings.appearance.presets')}</Heading>
					<div class="flex gap-2 flex-wrap">
						{#each Object.keys(colorPresets) as id}
							<button
								type="button"
								class="flex items-center gap-2 px-3 py-1.5 rounded border border-gray-600 bg-gray-900 text-xs text-gray-300 hover:border-gray-400"
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
		</Card>
		<div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-4">{$t('settings.appearance.colorScheme')}</Heading>

				<div class="mb-3 border-b border-gray-700 pb-3">
					<h4 class="text-xs font-semibold text-blue-400 mb-2">{$t('settings.appearance.issuePrStatus')}</h4>
					<div class="space-y-1.5">
						{#each [['noPr', $t('settings.appearance.noPr')], ['hasPr', $t('settings.appearance.hasPr')], ['prReady', $t('settings.appearance.prReady')]] as [key, label]}
							<label class="flex items-center gap-2">
								<input type="color" bind:value={colors[key]} onchange={saveColors} class="w-6 h-5 rounded border border-gray-600 bg-transparent cursor-pointer" />
								<span class="text-xs text-gray-300">{label}</span>
								<span class="ml-auto text-[0.6rem] mono text-gray-600">{colors[key]}</span>
							</label>
						{/each}
					</div>
				</div>

				<div class="mb-3 border-b border-gray-700 pb-3">
					<h4 class="text-xs font-semibold text-blue-400 mb-2">{$t('settings.appearance.priority')}</h4>
					<div class="space-y-1.5">
						{#each [['critical', $t('settings.appearance.critical')], ['high', $t('settings.appearance.high')], ['medium', $t('settings.appearance.medium')], ['low', $t('settings.appearance.low')]] as [key, label]}
							<label class="flex items-center gap-2">
								<input type="color" bind:value={colors[key]} onchange={saveColors} class="w-6 h-5 rounded border border-gray-600 bg-transparent cursor-pointer" />
								<span class="text-xs text-gray-300">{label}</span>
								<span class="ml-auto text-[0.6rem] mono text-gray-600">{colors[key]}</span>
							</label>
						{/each}
					</div>
				</div>

				<div class="mb-3 border-b border-gray-700 pb-3">
					<h4 class="text-xs font-semibold text-blue-400 mb-2">{$t('settings.appearance.riskCategory')}</h4>
					<div class="space-y-1.5">
						{#each [['riskHigh', $t('settings.appearance.riskHigh')], ['riskMedium', $t('settings.appearance.riskMedium')], ['riskLow', $t('settings.appearance.riskLow')], ['bug', $t('settings.appearance.bug')], ['feature', $t('settings.appearance.feature')], ['docs', $t('settings.appearance.docs')]] as [key, label]}
							<label class="flex items-center gap-2">
								<input type="color" bind:value={colors[key]} onchange={saveColors} class="w-6 h-5 rounded border border-gray-600 bg-transparent cursor-pointer" />
								<span class="text-xs text-gray-300">{label}</span>
								<span class="ml-auto text-[0.6rem] mono text-gray-600">{colors[key]}</span>
							</label>
						{/each}
					</div>
				</div>

				<Button onclick={resetColors} color="alternative" size="xs">{$t('settings.appearance.reset')}</Button>
			</Card>

			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-4">{$t('settings.appearance.legend')}</Heading>
				<div class="grid grid-cols-2 gap-4 text-xs text-gray-300">
					<div>
						<h4 class="text-gray-500 mb-1 text-[0.6rem] uppercase">{$t('settings.appearance.legend.prStatus')}</h4>
						{#each [['noPr', $t('settings.appearance.noPr')], ['hasPr', $t('settings.appearance.hasPr')], ['prReady', $t('settings.appearance.prReady')]] as [key, label]}
							<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded inline-block" style="background: {colors[key]}"></span> {label}</div>
						{/each}
					</div>
					<div>
						<h4 class="text-gray-500 mb-1 text-[0.6rem] uppercase">{$t('settings.appearance.legend.priority')}</h4>
						{#each [['critical', $t('settings.appearance.critical')], ['high', $t('settings.appearance.high')], ['medium', $t('settings.appearance.medium')], ['low', $t('settings.appearance.low')]] as [key, label]}
							<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded inline-block" style="background: {colors[key]}"></span> {label}</div>
						{/each}
					</div>
					<div>
						<h4 class="text-gray-500 mb-1 text-[0.6rem] uppercase">{$t('settings.appearance.legend.risk')}</h4>
						{#each [['riskHigh', $t('settings.appearance.high')], ['riskMedium', $t('settings.appearance.medium')], ['riskLow', $t('settings.appearance.low')]] as [key, label]}
							<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded inline-block" style="background: {colors[key]}"></span> {label}</div>
						{/each}
					</div>
					<div>
						<h4 class="text-gray-500 mb-1 text-[0.6rem] uppercase">{$t('settings.appearance.legend.category')}</h4>
						{#each [['bug', $t('settings.appearance.bug')], ['feature', $t('settings.appearance.feature')], ['docs', $t('settings.appearance.docs')]] as [key, label]}
							<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded inline-block" style="background: {colors[key]}"></span> {label}</div>
						{/each}
					</div>
				</div>
			</Card>
		</div>
	</TabItem>

	<!-- ========================= CONFIGURATION ========================= -->
	<TabItem title={$t('settings.tabs.configuration')}>
		<Card class="bg-gray-800 border-gray-700 max-w-none">
			<Heading tag="h3" class="text-base mb-4">{$t('settings.config.title')}</Heading>
			<Helper class="mb-4">
				{$t('settings.config.helper.prefix')} <code class="rounded bg-gray-700 px-1 py-0.5">.wshm/config.toml</code>{$t('settings.config.helper.suffix')}
			</Helper>

			<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
				{#each [
					[$t('settings.config.section.triage'), [['Enabled', 'true'], ['Auto-fix', 'false'], ['Confidence', '0.85']]],
					[$t('settings.config.section.prAnalysis'), [['Enabled', 'true'], ['Auto-label', 'true'], ['Risk labels', 'true']]],
					[$t('settings.config.section.mergeQueue'), [['Threshold', '15'], ['Strategy', 'rebase']]],
					[$t('settings.config.section.sync'), [['Interval', '5 min'], ['Full sync', '24h']]]
				] as [section, items]}
					<div class="border border-gray-700 rounded p-3">
						<h4 class="text-xs font-semibold text-blue-400 mb-2">{section}</h4>
						<dl class="grid grid-cols-[120px_1fr] gap-x-2 gap-y-0.5">
							{#each items as [key, val]}
								<dt class="text-xs text-gray-500">{key}</dt>
								<dd class="text-xs text-gray-300 mono">{val}</dd>
							{/each}
						</dl>
					</div>
				{/each}
			</div>
		</Card>
	</TabItem>

	<!-- ========================= RELIABILITY ========================= -->
	<TabItem title={$t('settings.tabs.reliability')}>
		<div class="w-full">
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-1">{$t('settings.retry.title')}</Heading>
				<Helper class="mb-4">{$t('settings.retry.helper')}</Helper>

				{#if retrySettings}
					{#if retryMessage}
						<Alert color={retryError ? 'red' : 'green'} class="text-xs py-2 mb-3">{retryMessage}</Alert>
					{/if}

					<form onsubmit={(e) => { e.preventDefault(); handleSaveRetry(); }} class="space-y-4 max-w-md">
						<div class="flex items-center justify-between">
							<Label class="text-xs">{$t('settings.retry.enabled')}</Label>
							<Toggle bind:checked={retrySettings.enabled} disabled={savingRetry} />
						</div>

						<div>
							<Label for="retry-attempts" class="text-xs mb-1">{$t('settings.retry.maxAttempts')}</Label>
							<Input id="retry-attempts" type="number" min="1" max="10" bind:value={retrySettings.max_attempts} disabled={savingRetry || !retrySettings.enabled} size="sm" />
							<Helper class="text-xs mt-1">{$t('settings.retry.maxAttemptsHelp')}</Helper>
						</div>

						<div>
							<Label for="retry-initial" class="text-xs mb-1">{$t('settings.retry.initialBackoff')}</Label>
							<Input id="retry-initial" type="number" min="50" max="60000" step="50" bind:value={retrySettings.initial_backoff_ms} disabled={savingRetry || !retrySettings.enabled} size="sm" />
							<Helper class="text-xs mt-1">{$t('settings.retry.initialBackoffHelp')}</Helper>
						</div>

						<div>
							<Label for="retry-max" class="text-xs mb-1">{$t('settings.retry.maxBackoff')}</Label>
							<Input id="retry-max" type="number" min="50" max="120000" step="100" bind:value={retrySettings.max_backoff_ms} disabled={savingRetry || !retrySettings.enabled} size="sm" />
							<Helper class="text-xs mt-1">{$t('settings.retry.maxBackoffHelp')}</Helper>
						</div>

						<Button type="submit" color="blue" disabled={savingRetry} size="sm" class="w-full">
							{savingRetry ? $t('common.saving') : $t('settings.retry.save')}
						</Button>
					</form>
				{:else}
					<p class="text-sm text-gray-500">{$t('common.loading')}</p>
				{/if}
			</Card>
		</div>
	</TabItem>

	<!-- ========================= SECRETS ============================ -->
	<TabItem title={$t('settings.tabs.secrets')}>
		<!-- Disambiguation banner: this tab is for advanced / per-repo
		     secrets. Common GitHub / Anthropic tokens belong in their
		     dedicated tabs. -->
		<Alert color="blue" class="mb-4 text-sm">
			<span class="font-semibold">{$t('settings.secrets.banner.title')}</span>
			{$t('settings.secrets.banner.body')}
		</Alert>

		<!-- Doc / how-to: create a github_token. Toggleable so admins
		     who already know the drill don't see it every visit. -->
		<details class="mb-4 rounded border border-gray-700 bg-gray-800/60 open:bg-gray-800">
			<summary class="cursor-pointer px-4 py-3 text-sm font-semibold text-blue-300 hover:text-blue-200">
				ℹ️ {translate('secrets.help.title')}
			</summary>
			<div class="px-4 pb-4 pt-1 text-sm text-gray-300 space-y-2">
				<p>{translate('secrets.help.intro')}</p>
				<ol class="list-decimal list-inside space-y-1 ms-2">
					<li>{translate('secrets.help.step1')}</li>
					<li>{translate('secrets.help.step2')}</li>
					<li>{translate('secrets.help.step3')}</li>
					<li>{translate('secrets.help.step4')}</li>
				</ol>
				<p class="text-xs text-gray-400 italic">
					💡 {translate('secrets.help.tip')}
				</p>
				<a
					href="https://github.com/settings/tokens"
					target="_blank"
					rel="noopener noreferrer"
					class="inline-block mt-1 text-blue-400 hover:text-blue-300 underline text-xs"
				>
					→ {translate('secrets.help.link')}
				</a>
			</div>
		</details>

		<div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
			<!-- Stored secrets list -->
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-4">{$t('settings.secrets.stored')}</Heading>
				<Helper class="mb-3">
					{$t('settings.secrets.encrypted')}
				</Helper>
				{#if secretsError}
					<Alert color="red" class="text-xs py-2 mb-2">{secretsError}</Alert>
				{/if}
				{#if secrets.length === 0}
					<p class="text-sm text-gray-500">{$t('settings.secrets.none')}</p>
				{:else}
					<Table hoverable={true} class="text-xs">
						<TableHead>
							<TableHeadCell>{$t('settings.secrets.col.scope')}</TableHeadCell>
							<TableHeadCell>{$t('settings.secrets.col.key')}</TableHeadCell>
							<TableHeadCell>{$t('settings.secrets.col.value')}</TableHeadCell>
							<TableHeadCell>{$t('settings.secrets.col.updated')}</TableHeadCell>
							<TableHeadCell><span class="sr-only">{$t('settings.secrets.actions')}</span></TableHeadCell>
						</TableHead>
						<TableBody>
							{#each secrets as s (s.id)}
								<TableBodyRow>
									<TableBodyCell>
										<Badge color={s.scope === 'global' ? 'blue' : 'green'}>
											{s.scope}{s.slug ? `: ${s.slug}` : ''}
										</Badge>
									</TableBodyCell>
									<TableBodyCell class="mono text-gray-200">{s.key}</TableBodyCell>
									<TableBodyCell class="mono text-gray-300">
										{revealedId === s.id && revealedValue ? revealedValue : '••••••••'}
									</TableBodyCell>
									<TableBodyCell class="text-gray-500">
										{new Date(s.updated_at).toLocaleString()}
									</TableBodyCell>
									<TableBodyCell class="text-right whitespace-nowrap">
										<Button color="alternative" size="xs" onclick={() => handleReveal(s.id)}>
											{revealedId === s.id ? $t('settings.secrets.hide') : $t('settings.secrets.reveal')}
										</Button>
										<Button color="red" size="xs" onclick={() => handleDeleteSecret(s.id)}>
											{$t('common.delete')}
										</Button>
									</TableBodyCell>
								</TableBodyRow>
							{/each}
						</TableBody>
					</Table>
				{/if}
			</Card>

			<!-- Add new secret -->
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-4">{$t('settings.secrets.add')}</Heading>
				{#if secretMessage}
					<Alert color={secretMessageErr ? 'red' : 'green'} class="text-xs py-2 mb-2">
						{secretMessage}
					</Alert>
				{/if}
				<form onsubmit={(e) => { e.preventDefault(); handleAddSecret(); }} class="space-y-3">
					<div>
						<Label class="text-xs mb-1">{$t('settings.secrets.scope')}</Label>
						<div class="flex gap-3 text-sm">
							<Radio bind:group={newSecretScope} value="global">{$t('settings.secrets.scope.global')}</Radio>
							<Radio bind:group={newSecretScope} value="repo">{$t('settings.secrets.scope.repo')}</Radio>
						</div>
					</div>
					{#if newSecretScope === 'repo'}
						<div>
							<Label for="sec-slug" class="text-xs mb-1">{$t('settings.secrets.repoSlug')}</Label>
							<Input id="sec-slug" type="text" bind:value={newSecretSlug}
								placeholder="owner/repo" disabled={savingSecret} size="sm" />
						</div>
					{/if}
					<div>
						<Label for="sec-key" class="text-xs mb-1">{$t('settings.secrets.key')}</Label>
						<Input id="sec-key" type="text" bind:value={newSecretKey}
							placeholder="github_token, anthropic_api_key, …"
							disabled={savingSecret} size="sm" />
						<Helper class="text-xs mt-1">
							{$t('settings.secrets.commonKeys')} <code>github_token</code>, <code>anthropic_oauth_token</code>,
							<code>anthropic_api_key</code>.
						</Helper>
					</div>
					<div>
						<Label for="sec-value" class="text-xs mb-1">{$t('settings.secrets.value')}</Label>
						<Input id="sec-value" type="password" bind:value={newSecretValue}
							placeholder="paste secret value" disabled={savingSecret} size="sm" />
					</div>
					<Button type="submit" color="blue" size="sm" class="w-full"
						disabled={savingSecret || !newSecretKey.trim() || !newSecretValue
							|| (newSecretScope === 'repo' && !newSecretSlug.trim())}>
						{savingSecret ? $t('common.saving') : $t('settings.secrets.save')}
					</Button>
				</form>
			</Card>
		</div>
	</TabItem>

	<!-- ========================= USERS (RBAC) ========================= -->
	<TabItem title={$t('settings.tabs.users')}>
		<Card class="bg-gray-800 border-gray-700 max-w-none">
			<div class="flex items-start justify-between mb-4 gap-3">
				<div>
					<Heading tag="h3" class="text-base">{$t('settings.users.title')}</Heading>
					<Helper class="mt-1">
						{$t('settings.users.helper')}
					</Helper>
				</div>
				<Button color="blue" size="sm" onclick={openCreateUser} class="shrink-0">
					{$t('settings.users.addUser')}
				</Button>
			</div>
			{#if usersError}
				<Alert color="red" class="text-xs py-2 mb-2">{usersError}</Alert>
			{/if}
			{#if userMessage}
				<Alert color={userMessageErr ? 'red' : 'green'} class="text-xs py-2 mb-2">
					{userMessage}
				</Alert>
			{/if}
			{#if users.length === 0}
				<p class="text-sm text-gray-500">{$t('settings.users.none')}</p>
			{:else}
				<Table hoverable={true} class="text-xs">
					<TableHead>
						<TableHeadCell>{$t('settings.users.col.identity')}</TableHeadCell>
						<TableHeadCell>{$t('settings.users.col.auth')}</TableHeadCell>
						<TableHeadCell>{$t('settings.users.col.role')}</TableHeadCell>
						<TableHeadCell>{$t('settings.users.col.lastLogin')}</TableHeadCell>
						<TableHeadCell><span class="sr-only">{$t('settings.secrets.actions')}</span></TableHeadCell>
					</TableHead>
					<TableBody>
						{#each users as u (u.id)}
							<TableBodyRow>
								<TableBodyCell>
									<div class="mono text-gray-200">{u.email}</div>
									{#if u.username && u.username !== u.email}
										<div class="text-[0.65rem] text-gray-500">@{u.username}</div>
									{/if}
								</TableBodyCell>
								<TableBodyCell>
									<Badge color={u.sso_provider ? 'purple' : 'blue'}>
										{u.sso_provider ?? 'local'}
									</Badge>
								</TableBodyCell>
								<TableBodyCell>
									<Badge color={u.role === 'admin' ? 'red' : u.role === 'operator' ? 'orange' : u.role === 'member' ? 'blue' : 'gray'}>
										{u.role}
									</Badge>
								</TableBodyCell>
								<TableBodyCell class="text-gray-500">
									{u.last_login_at ? new Date(u.last_login_at).toLocaleString() : '—'}
								</TableBodyCell>
								<TableBodyCell class="text-right whitespace-nowrap">
									<Button color="alternative" size="xs" onclick={() => openEditUser(u)}>
										{$t('common.edit')}
									</Button>
									<Button color="red" size="xs" onclick={() => openDeleteUser(u)}>
										{$t('common.delete')}
									</Button>
								</TableBodyCell>
							</TableBodyRow>
						{/each}
					</TableBody>
				</Table>
			{/if}
		</Card>
	</TabItem>
</Tabs>

<!-- Create user modal -->
<Modal
	bind:open={createUserModalOpen}
	title={$t('settings.users.modal.create.title')}
	size="md"
	dismissable
	class="bg-gray-900 border-gray-700"
	bodyClass="text-gray-200"
>
	<form onsubmit={(e) => { e.preventDefault(); handleCreateUser(); }} class="space-y-3">
		<div>
			<Label for="user-email" class="text-xs mb-1">{$t('settings.users.email')}</Label>
			<Input id="user-email" type="text" bind:value={newUserEmail}
				placeholder="alice@example.com or alice" disabled={creatingUser} size="sm" />
		</div>
		<div>
			<Label for="user-username" class="text-xs mb-1">{$t('settings.users.username')}</Label>
			<Input id="user-username" type="text" bind:value={newUserUsername}
				placeholder="alice" disabled={creatingUser} size="sm" />
		</div>
		<div>
			<Label for="user-password" class="text-xs mb-1">{$t('settings.users.password')}</Label>
			<Input id="user-password" type="password" bind:value={newUserPassword}
				placeholder={$t('settings.users.password.placeholder')} disabled={creatingUser} size="sm" />
		</div>
		<div>
			<Label class="text-xs mb-1">{$t('settings.users.role')}</Label>
			<div class="flex flex-col gap-1 text-sm">
				<Radio bind:group={newUserRole} value="admin">
					<span class="font-semibold">{$t('settings.users.role.admin')}</span>
					<span class="text-xs text-gray-500 ml-1">{$t('settings.users.role.admin.help')}</span>
				</Radio>
				<Radio bind:group={newUserRole} value="operator">
					<span class="font-semibold">{$t('settings.users.role.operator')}</span>
					<span class="text-xs text-gray-500 ml-1">{$t('settings.users.role.operator.help')}</span>
				</Radio>
				<Radio bind:group={newUserRole} value="member">
					<span class="font-semibold">{$t('settings.users.role.member')}</span>
					<span class="text-xs text-gray-500 ml-1">{$t('settings.users.role.member.help')}</span>
				</Radio>
				<Radio bind:group={newUserRole} value="viewer">
					<span class="font-semibold">{$t('settings.users.role.viewer')}</span>
					<span class="text-xs text-gray-500 ml-1">{$t('settings.users.role.viewer.help')}</span>
				</Radio>
			</div>
		</div>
		<div class="flex gap-2 pt-2">
			<Button color="alternative" size="sm" class="flex-1"
				onclick={() => createUserModalOpen = false} disabled={creatingUser}>
				{$t('common.cancel')}
			</Button>
			<Button type="submit" color="blue" size="sm" class="flex-1"
				disabled={creatingUser || !newUserEmail.trim() || !newUserPassword || newUserPassword.length < 6}>
				{creatingUser ? $t('settings.users.creating') : $t('settings.users.create')}
			</Button>
		</div>
	</form>
</Modal>

<!-- Edit user modal -->
<Modal
	bind:open={editUserModalOpen}
	title={editingUser ? `${$t('settings.users.modal.edit.titlePrefix')} ${editingUser.email}` : $t('settings.users.modal.edit.titleFallback')}
	size="md"
	dismissable
	class="bg-gray-900 border-gray-700"
	bodyClass="text-gray-200"
>
	{#if editingUser}
		<form onsubmit={(e) => { e.preventDefault(); handleSaveEdit(); }} class="space-y-3">
			<div>
				<Label class="text-xs mb-1">{$t('settings.users.role')}</Label>
				<div class="flex flex-col gap-1 text-sm">
					<Radio bind:group={editRole} value="admin">
						<span class="font-semibold">{$t('settings.users.role.admin')}</span>
						<span class="text-xs text-gray-500 ml-1">{$t('settings.users.role.admin.help.short')}</span>
					</Radio>
					<Radio bind:group={editRole} value="operator">
						<span class="font-semibold">{$t('settings.users.role.operator')}</span>
						<span class="text-xs text-gray-500 ml-1">{$t('settings.users.role.operator.help')}</span>
					</Radio>
					<Radio bind:group={editRole} value="member">
						<span class="font-semibold">{$t('settings.users.role.member')}</span>
						<span class="text-xs text-gray-500 ml-1">{$t('settings.users.role.member.help')}</span>
					</Radio>
					<Radio bind:group={editRole} value="viewer">
						<span class="font-semibold">{$t('settings.users.role.viewer')}</span>
						<span class="text-xs text-gray-500 ml-1">{$t('settings.users.role.viewer.help')}</span>
					</Radio>
				</div>
			</div>
			<div>
				<Label for="edit-pw" class="text-xs mb-1">{$t('settings.users.newPassword')}</Label>
				<Input id="edit-pw" type="password" bind:value={editPassword}
					placeholder={$t('settings.users.password.placeholder')} disabled={savingEdit} size="sm" />
			</div>
			<div class="flex gap-2 pt-2">
				<Button color="alternative" size="sm" class="flex-1"
					onclick={() => editUserModalOpen = false} disabled={savingEdit}>
					{$t('common.cancel')}
				</Button>
				<Button type="submit" color="blue" size="sm" class="flex-1" disabled={savingEdit}>
					{savingEdit ? $t('common.saving') : $t('common.save')}
				</Button>
			</div>
		</form>
	{/if}
</Modal>

<!-- Edit features modal -->
<Modal
	bind:open={featuresModalOpen}
	title={featuresSlug ? `${$t('settings.features.modalTitleFor')} ${featuresSlug}` : $t('settings.features.modalTitle')}
	size="lg"
	dismissable
	class="bg-gray-900 border-gray-700"
	bodyClass="text-gray-200"
>
	{#if featuresMessage}
		<Alert color={featuresMessageErr ? 'red' : 'green'} class="text-xs py-2 mb-3">
			{featuresMessage}
		</Alert>
	{/if}
	{#if !featuresDraft}
		<p class="text-sm text-gray-500">{$t('common.loading')}</p>
	{:else}
		<div class="space-y-4">
			<!-- Master mode: dry-run vs apply. Switches all write-back actions. -->
			<div
				class="rounded-lg border p-3 transition-colors {featuresDraft.apply
					? 'border-green-700/60 bg-green-900/20'
					: 'border-yellow-700/60 bg-yellow-900/20'}"
			>
				<div class="flex items-center justify-between gap-3">
					<div>
						<h4 class="text-sm font-semibold {featuresDraft.apply ? 'text-green-300' : 'text-yellow-300'} flex items-center gap-1">
							{featuresDraft.apply
								? $t('settings.features.mode.apply')
								: $t('settings.features.mode.dryrun')}
							{@render infoTip('mode-tip', 'settings.features.mode.tip')}
						</h4>
						<p class="text-xs text-gray-400 mt-0.5">
							{featuresDraft.apply
								? $t('settings.features.mode.body.apply')
								: $t('settings.features.mode.body.dryrun')}
						</p>
					</div>
					<label class="inline-flex items-center cursor-pointer shrink-0">
						<input
							type="checkbox"
							bind:checked={featuresDraft.apply}
							class="sr-only peer"
						/>
						<span class="relative w-11 h-6 bg-gray-600 peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-blue-500/40 rounded-full peer peer-checked:bg-green-600 transition-colors after:content-[''] after:absolute after:top-0.5 after:left-0.5 after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-transform peer-checked:after:translate-x-5"></span>
					</label>
				</div>
			</div>

			<div>
				<h4 class="text-xs uppercase text-gray-500 font-semibold mb-2">{$t('settings.features.collection.title')}</h4>
				<p class="text-xs text-gray-500 mb-2">
					{$t('settings.features.collection.body')}
				</p>
				<div class="space-y-1.5">
					<label class="flex items-center gap-2 text-sm">
						<input type="checkbox" bind:checked={featuresDraft.collect_issues} class="rounded" />
						<span><strong>{$t('settings.features.collection.issues')}</strong> <span class="text-xs text-gray-500">{$t('settings.features.collection.issues.help')}</span></span>
						{@render infoTip('tip-collect-issues', 'settings.features.collection.issues.tip')}
					</label>
					<label class="flex items-center gap-2 text-sm">
						<input type="checkbox" bind:checked={featuresDraft.collect_prs} class="rounded" />
						<span><strong>{$t('settings.features.collection.prs')}</strong> <span class="text-xs text-gray-500">{$t('settings.features.collection.prs.help')}</span></span>
						{@render infoTip('tip-collect-prs', 'settings.features.collection.prs.tip')}
					</label>
				</div>
			</div>

			<div class:opacity-60={!featuresDraft.apply}>
				<h4 class="text-xs uppercase text-gray-500 font-semibold mb-2">
					{$t('settings.features.ai.title')}
					{#if !featuresDraft.apply}
						<span class="ml-2 text-yellow-500/80 normal-case font-normal">{$t('settings.features.ai.dimmed')}</span>
					{/if}
				</h4>
				<p class="text-xs text-gray-500 mb-2">
					{$t('settings.features.ai.body')}
				</p>
				<div class="space-y-1.5">
					<label class="flex items-center gap-2 text-sm">
						<input type="checkbox" bind:checked={featuresDraft.triage_issues} class="rounded" />
						<span><strong>{$t('settings.features.ai.triage')}</strong> <span class="text-xs text-gray-500">{$t('settings.features.ai.triage.help')}</span></span>
						{@render infoTip('tip-triage', 'settings.features.ai.triage.tip')}
					</label>
					<label class="flex items-center gap-2 text-sm">
						<input type="checkbox" bind:checked={featuresDraft.analyze_prs} class="rounded" />
						<span><strong>{$t('settings.features.ai.analyze')}</strong> <span class="text-xs text-gray-500">{$t('settings.features.ai.analyze.help')}</span></span>
						{@render infoTip('tip-analyze', 'settings.features.ai.analyze.tip')}
					</label>
					<label class="flex items-center gap-2 text-sm">
						<input type="checkbox" bind:checked={featuresDraft.review_prs} class="rounded" />
						<span>
							<strong>{$t('settings.features.ai.review')}</strong>
							<span class="text-xs text-gray-500">{$t('settings.features.ai.review.help')}</span>
						</span>
						{@render infoTip('tip-review', 'settings.features.ai.review.tip')}
					</label>
				</div>
			</div>

			<div class:opacity-60={!featuresDraft.apply}>
				<h4 class="text-xs uppercase text-gray-500 font-semibold mb-2">
					{$t('settings.features.auto.title')}
					{#if !featuresDraft.apply}
						<span class="ml-2 text-yellow-500/80 normal-case font-normal">{$t('settings.features.auto.dimmed')}</span>
					{/if}
				</h4>
				<p class="text-xs text-gray-500 mb-2">
					{$t('settings.features.auto.body')}
				</p>
				<div class="space-y-1.5">
					<label class="flex items-center gap-2 text-sm">
						<input type="checkbox" bind:checked={featuresDraft.auto_pr} class="rounded" />
						<span><strong>{$t('settings.features.auto.fix')}</strong> <span class="text-xs text-gray-500">{$t('settings.features.auto.fix.help')}</span></span>
						{@render infoTip('tip-autopr', 'settings.features.auto.fix.tip')}
					</label>
					<label class="flex items-center gap-2 text-sm">
						<input type="checkbox" bind:checked={featuresDraft.auto_merge} class="rounded" />
						<span><strong>{$t('settings.features.auto.merge')}</strong> <span class="text-xs text-gray-500">{$t('settings.features.auto.merge.help')}</span></span>
						{@render infoTip('tip-automerge', 'settings.features.auto.merge.tip')}
					</label>
				</div>
			</div>

			<!-- Advanced filters: collapsible. Free-text comma-separated for arrays. -->
			<details class="rounded border border-gray-700 bg-gray-900/40">
				<summary class="cursor-pointer px-3 py-2 text-sm font-semibold text-blue-300 hover:text-blue-200">
					{$t('settings.advancedFilters')}
				</summary>
				<div class="p-3 space-y-3 text-sm">
					<!-- One-click defaults aligned with GitHub's standard label set. -->
					<div class="flex items-start justify-between gap-3 rounded border border-blue-700/40 bg-blue-900/20 p-3">
						<div class="text-xs">
							<p class="font-semibold text-blue-300 mb-1">{$t('settings.advancedFilters.defaults.title')}</p>
							<p class="text-gray-400">
								{$t('settings.advancedFilters.defaults.body')}
							</p>
						</div>
						<Button color="blue" size="xs" onclick={applyGithubDefaults} class="shrink-0">
							{$t('settings.advancedFilters.defaults.apply')}
						</Button>
					</div>

					<details class="rounded border border-gray-700 bg-gray-900/40">
						<summary class="cursor-pointer px-3 py-2 text-xs font-semibold text-gray-400 hover:text-gray-200">
							{$t('settings.advancedFilters.defaults.help')}
						</summary>
						<div class="p-3 text-xs space-y-1 text-gray-400">
							<div><code class="text-red-300">bug</code> — Something isn't working. <em>Triage candidate.</em></div>
							<div><code class="text-cyan-300">enhancement</code> — New feature or request.</div>
							<div><code class="text-blue-300">documentation</code> — Doc improvements.</div>
							<div><code class="text-yellow-300">good first issue</code> — Good for newcomers. <em>Auto-fix candidate.</em></div>
							<div><code class="text-green-300">help wanted</code> — Extra attention is needed.</div>
							<div><code class="text-purple-300">question</code> — Further info requested. <em>Skip triage (human judgment).</em></div>
							<div><code class="text-gray-500">duplicate</code> — Already exists. <em>Skip triage.</em></div>
							<div><code class="text-gray-500">invalid</code> — Doesn't seem right. <em>Skip triage.</em></div>
							<div><code class="text-gray-500">wontfix</code> — Will not be worked on. <em>Skip triage.</em></div>
						</div>
					</details>
					<div>
						<h5 class="text-xs uppercase text-gray-500 font-semibold mb-1">{$t('settings.advancedFilters.section.global')}</h5>
						<Label class="text-xs mb-1">{$t('settings.advancedFilters.skipAuthors')}</Label>
						<Input
							size="sm"
							placeholder="dependabot[bot], renovate[bot]"
							value={featuresDraft.filters.skip_authors.join(', ')}
							onchange={(e) => {
								featuresDraft!.filters.skip_authors = parseCsv((e.currentTarget as HTMLInputElement).value);
							}}
						/>
						<Label class="text-xs mb-1 mt-2">{$t('settings.advancedFilters.targetBranches')}</Label>
						<Input
							size="sm"
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
						<h5 class="text-xs uppercase text-gray-500 font-semibold mb-1">{$t('settings.advancedFilters.section.triage')}</h5>
						<Label class="text-xs mb-1">{$t('settings.advancedFilters.onlyLabels')}</Label>
						<Input
							size="sm"
							placeholder="needs-triage, bug"
							value={featuresDraft.filters.triage_only_labels.join(', ')}
							onchange={(e) => {
								featuresDraft!.filters.triage_only_labels = parseCsv((e.currentTarget as HTMLInputElement).value);
							}}
						/>
						<Label class="text-xs mb-1 mt-2">{$t('settings.advancedFilters.skipLabels')}</Label>
						<Input
							size="sm"
							placeholder="wontfix, duplicate"
							value={featuresDraft.filters.triage_skip_labels.join(', ')}
							onchange={(e) => {
								featuresDraft!.filters.triage_skip_labels = parseCsv((e.currentTarget as HTMLInputElement).value);
							}}
						/>
						<Label class="text-xs mb-1 mt-2">{$t('settings.advancedFilters.maxAge')}</Label>
						<Input
							type="number"
							size="sm"
							bind:value={featuresDraft.filters.triage_max_age_days}
						/>
					</div>

					<div>
						<h5 class="text-xs uppercase text-gray-500 font-semibold mb-1">{$t('settings.advancedFilters.section.analyze')}</h5>
						<div class="grid grid-cols-2 gap-2">
							<div>
								<Label class="text-xs mb-1">{$t('settings.advancedFilters.minLoc')}</Label>
								<Input type="number" size="sm" bind:value={featuresDraft.filters.analyze_min_loc} />
							</div>
							<div>
								<Label class="text-xs mb-1">{$t('settings.advancedFilters.maxLoc')}</Label>
								<Input type="number" size="sm" bind:value={featuresDraft.filters.analyze_max_loc} />
							</div>
						</div>
					</div>

					<div>
						<h5 class="text-xs uppercase text-gray-500 font-semibold mb-1">{$t('settings.advancedFilters.section.autoFix')}</h5>
						<Label class="text-xs mb-1">{$t('settings.advancedFilters.onlyLabels')}</Label>
						<Input
							size="sm"
							placeholder="good-first-issue, auto-fix"
							value={featuresDraft.filters.auto_pr_only_labels.join(', ')}
							onchange={(e) => {
								featuresDraft!.filters.auto_pr_only_labels = parseCsv((e.currentTarget as HTMLInputElement).value);
							}}
						/>
						<Label class="text-xs mb-1 mt-2">{$t('settings.advancedFilters.targetBranch')}</Label>
						<Input size="sm" placeholder="main" bind:value={featuresDraft.filters.auto_pr_target_branch} />
					</div>

					<div>
						<h5 class="text-xs uppercase text-gray-500 font-semibold mb-1">{$t('settings.advancedFilters.section.autoMerge')}</h5>
						<Label class="text-xs mb-1">{$t('settings.advancedFilters.onlyAuthors')}</Label>
						<Input
							size="sm"
							placeholder="dependabot[bot]"
							value={featuresDraft.filters.auto_merge_only_authors.join(', ')}
							onchange={(e) => {
								featuresDraft!.filters.auto_merge_only_authors = parseCsv((e.currentTarget as HTMLInputElement).value);
							}}
						/>
						<Label class="text-xs mb-1 mt-2">{$t('settings.advancedFilters.onlyLabels')}</Label>
						<Input
							size="sm"
							placeholder="auto-merge"
							value={featuresDraft.filters.auto_merge_only_labels.join(', ')}
							onchange={(e) => {
								featuresDraft!.filters.auto_merge_only_labels = parseCsv((e.currentTarget as HTMLInputElement).value);
							}}
						/>
						<div class="grid grid-cols-2 gap-2 mt-2">
							<div>
								<Label class="text-xs mb-1">{$t('settings.advancedFilters.minApprovals')}</Label>
								<Input type="number" size="sm" bind:value={featuresDraft.filters.auto_merge_min_approvals} />
							</div>
							<div>
								<Label class="text-xs mb-1">{$t('settings.advancedFilters.maxLoc')}</Label>
								<Input type="number" size="sm" bind:value={featuresDraft.filters.auto_merge_max_loc} />
							</div>
						</div>
					</div>
				</div>
			</details>

			<div class="flex gap-2 pt-2">
				<Button color="alternative" size="sm" class="flex-1"
					onclick={() => featuresModalOpen = false} disabled={featuresSaving}>
					{$t('common.cancel')}
				</Button>
				<Button color="blue" size="sm" class="flex-1"
					onclick={handleSaveFeatures} disabled={featuresSaving}>
					{featuresSaving ? $t('common.saving') : $t('common.save')}
				</Button>
			</div>
		</div>
	{/if}
</Modal>

<!-- Delete user confirm modal -->
<Modal
	bind:open={deleteUserModalOpen}
	title={$t('settings.users.modal.delete.title')}
	size="sm"
	dismissable
	class="bg-gray-900 border-gray-700"
	bodyClass="text-gray-200"
>
	{#if deletingUser}
		<p class="text-sm">
			{$t('settings.users.modal.delete.prefix')} <span class="mono text-red-300">{deletingUser.email}</span>{$t('settings.users.modal.delete.confirm')}
		</p>
		<div class="flex gap-2 pt-4">
			<Button color="alternative" size="sm" class="flex-1"
				onclick={() => deleteUserModalOpen = false} disabled={deletingNow}>
				{$t('common.cancel')}
			</Button>
			<Button color="red" size="sm" class="flex-1"
				onclick={handleConfirmDelete} disabled={deletingNow}>
				{deletingNow ? $t('settings.users.deleting') : $t('common.delete')}
			</Button>
		</div>
	{/if}
</Modal>
