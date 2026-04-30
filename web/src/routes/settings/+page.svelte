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
	} from 'flowbite-svelte';
	import { colorConfig, type ColorConfig } from '$lib/colors';
	import {
		fetchLicense,
		activateLicense,
		fetchRepos,
		addRepo,
		fetchAuthStatus,
		setGithubToken,
		setAnthropicToken,
		type LicenseInfo,
		type ReposListResponse,
		type AuthStatus,
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

	function saveColors() { colorConfig.save(colors); }
	function resetColors() { colorConfig.reset(); colors = { ...colorConfig.defaults }; }

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

	onMount(async () => {
		try { license = await fetchLicense(); } catch { /* ignore */ }
		await refreshRepos();
		await refreshAuth();
	});
</script>

<svelte:head>
	<title>wshm - Settings</title>
</svelte:head>

<div class="mb-4">
	<Heading tag="h2" class="text-xl mb-1">Settings</Heading>
	<p class="text-sm text-gray-500">Connections, license, appearance, and configuration</p>
</div>

<Tabs tabStyle="underline" contentClass="bg-transparent p-0 mt-4">
	<!-- ========================= REPOSITORIES ========================= -->
	<TabItem open title="Repositories">
		<div class="w-full">
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-4">Repositories</Heading>

				{#if reposList}
					<div class="mb-3">
						<h4 class="text-xs font-semibold text-blue-400 mb-2">Configured ({reposList.repos.length})</h4>
						{#if reposList.repos.length === 0}
							<p class="text-xs text-gray-500">None configured.</p>
						{:else}
							<ul class="space-y-1 text-xs">
								{#each reposList.repos as r}
									<li class="flex items-center justify-between">
										<span class="text-gray-300 mono">{r.slug}</span>
										<Badge color={r.apply ? 'green' : 'dark'}>{r.apply ? 'apply' : 'dry-run'}</Badge>
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
									<Label for="repo-slug" class="text-xs mb-1">Slug</Label>
									<Input id="repo-slug" type="text" bind:value={newRepoSlug} placeholder="owner/repo" disabled={addingRepo} size="sm" />
								</div>
								<div>
									<Label for="repo-path" class="text-xs mb-1">Path (optional)</Label>
									<Input id="repo-path" type="text" bind:value={newRepoPath} placeholder="/abs/path" disabled={addingRepo} size="sm" />
								</div>
								<Button type="submit" color="blue" disabled={addingRepo || !newRepoSlug.trim()} size="sm" class="w-full">
									{addingRepo ? 'Adding...' : 'Add repository'}
								</Button>
							</form>
						{:else}
							<Helper>
								Dynamic add not available — daemon running mono-repo. Edit
								<code class="rounded bg-gray-700 px-1 py-0.5">~/.wshm/global.toml</code> and restart.
							</Helper>
						{/if}
					</div>
				{:else}
					<p class="text-sm text-gray-500">Loading...</p>
				{/if}
			</Card>
		</div>
	</TabItem>

	<!-- ========================= GIT PROVIDERS ========================= -->
	<TabItem title="Git providers">
		<div class="w-full">
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-4">GitHub authentication</Heading>

				{#if authStatus}
					<div class="mb-3">
						<Badge large color={authStatus.github ? 'green' : 'dark'}>
							{authStatus.github ? 'Configured' : 'Not configured'}
						</Badge>
					</div>

					<Helper class="mb-3">
						Personal Access Token. <a href="https://github.com/settings/tokens" target="_blank" class="text-blue-400 hover:underline">Generate one</a> with <code class="rounded bg-gray-700 px-1 py-0.5">repo</code> scope.
					</Helper>

					{#if ghMessage}
						<Alert color={ghError ? 'red' : 'green'} class="text-xs py-2 mb-2">{ghMessage}</Alert>
					{/if}

					<form onsubmit={(e) => { e.preventDefault(); handleSetGithub(); }} class="space-y-2">
						<div>
							<Label for="gh-token" class="text-xs mb-1">Token</Label>
							<Input id="gh-token" type="password" bind:value={ghToken} placeholder="ghp_..." disabled={savingGh} size="sm" />
						</div>
						<Button type="submit" color="blue" disabled={savingGh || !ghToken.trim()} size="sm" class="w-full">
							{savingGh ? 'Saving...' : 'Save token'}
						</Button>
					</form>
				{:else}
					<p class="text-sm text-gray-500">Loading...</p>
				{/if}
			</Card>
			<Helper class="mt-3 text-xs">More providers (GitLab, Gitea, Forgejo, Azure DevOps) coming soon.</Helper>
		</div>
	</TabItem>

	<!-- ========================= AI PROVIDERS ========================= -->
	<TabItem title="AI providers">
		<div class="w-full">
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-4">Claude / Anthropic authentication</Heading>

				{#if authStatus}
					<div class="mb-3">
						<Badge large color={authStatus.anthropic ? 'green' : 'dark'}>
							{authStatus.anthropic === 'oauth'
								? 'OAuth (Max/Pro)'
								: authStatus.anthropic === 'api_key'
									? 'API key'
									: 'Not configured'}
						</Badge>
					</div>

					<Helper class="mb-3">
						OAuth token (run <code class="rounded bg-gray-700 px-1 py-0.5">claude /token</code> in Claude Code) or an
						<a href="https://console.anthropic.com/" target="_blank" class="text-blue-400 hover:underline">API key</a>.
					</Helper>

					{#if anthropicMessage}
						<Alert color={anthropicError ? 'red' : 'green'} class="text-xs py-2 mb-2">{anthropicMessage}</Alert>
					{/if}

					<form onsubmit={(e) => { e.preventDefault(); handleSetAnthropic(); }} class="space-y-2">
						<div class="flex gap-4 text-xs">
							<Radio bind:group={anthropicKind} value="oauth" disabled={savingAnthropic}>OAuth</Radio>
							<Radio bind:group={anthropicKind} value="api_key" disabled={savingAnthropic}>API key</Radio>
						</div>
						<div>
							<Label for="anth-token" class="text-xs mb-1">Token</Label>
							<Input id="anth-token" type="password" bind:value={anthropicToken} placeholder={anthropicKind === 'oauth' ? 'sk-ant-oat01-...' : 'sk-ant-api03-...'} disabled={savingAnthropic} size="sm" />
						</div>
						<Button type="submit" color="blue" disabled={savingAnthropic || !anthropicToken.trim()} size="sm" class="w-full">
							{savingAnthropic ? 'Saving...' : 'Save token'}
						</Button>
					</form>
				{:else}
					<p class="text-sm text-gray-500">Loading...</p>
				{/if}
			</Card>
			<Helper class="mt-3 text-xs">More providers (OpenAI, Gemini, Ollama, Azure OpenAI) coming soon.</Helper>
		</div>
	</TabItem>

	<!-- ============================ LICENSE ============================ -->
	<TabItem title="License">
		<Card class="bg-gray-800 border-gray-700 max-w-none">
			<Heading tag="h3" class="text-base mb-4">License</Heading>

			{#if license}
				<div class="flex items-center gap-3 mb-4">
					<Badge large color={license.is_pro ? 'green' : 'dark'}>{license.plan.toUpperCase()}</Badge>
					{#if !license.is_pro}
						<span class="text-sm text-gray-400">Free tier</span>
					{/if}
				</div>

				<div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-4">
					<div>
						<h4 class="text-sm font-semibold text-gray-400 mb-2">OSS Features (included)</h4>
						<div class="flex flex-wrap gap-1">
							{#each license.oss_features as f}
								<Badge color="blue">{f}</Badge>
							{/each}
						</div>
					</div>
					<div>
						<h4 class="text-sm font-semibold text-gray-400 mb-2">Pro Features</h4>
						<div class="space-y-1">
							{#each license.features as f}
								<div class="flex items-center justify-between text-sm">
									<span class="text-gray-300">{f.label}</span>
									<Badge color={f.enabled ? 'green' : 'dark'}>{f.enabled ? 'Active' : 'Locked'}</Badge>
								</div>
							{/each}
						</div>
					</div>
				</div>

				<div class="border-t border-gray-700 pt-3">
					{#if activateMessage}
						<Alert color={activateError ? 'red' : 'green'} class="text-xs py-2 mb-2">{activateMessage}</Alert>
					{/if}

					<Helper class="mb-2">{license.is_pro ? 'Update license key:' : 'Enter your license key:'}</Helper>
					<form onsubmit={(e) => { e.preventDefault(); handleActivate(); }} class="flex gap-2">
						<Input type="text" bind:value={licenseKey} placeholder="wshm-pro-xxxx-xxxx-xxxx" disabled={activating} size="sm" class="flex-1" />
						<Button type="submit" color="blue" disabled={activating || !licenseKey.trim()} size="sm">
							{activating ? 'Activating...' : 'Activate'}
						</Button>
					</form>

					{#if !license.is_pro}
						<p class="text-xs text-gray-500 mt-2">
							<a href="https://wshm.dev/pro" target="_blank" class="text-blue-400 hover:underline">Get a license</a>
						</p>
					{/if}
				</div>
			{:else}
				<p class="text-sm text-gray-500">Loading...</p>
			{/if}
		</Card>
	</TabItem>

	<!-- ========================== APPEARANCE ========================== -->
	<TabItem title="Appearance">
		<div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-4">Color Scheme</Heading>

				<div class="mb-3 border-b border-gray-700 pb-3">
					<h4 class="text-xs font-semibold text-blue-400 mb-2">Issue PR Status</h4>
					<div class="space-y-1.5">
						{#each [['noPr', 'No PR'], ['hasPr', 'PR open'], ['prReady', 'PR ready']] as [key, label]}
							<label class="flex items-center gap-2">
								<input type="color" bind:value={colors[key]} onchange={saveColors} class="w-6 h-5 rounded border border-gray-600 bg-transparent cursor-pointer" />
								<span class="text-xs text-gray-300">{label}</span>
								<span class="ml-auto text-[0.6rem] mono text-gray-600">{colors[key]}</span>
							</label>
						{/each}
					</div>
				</div>

				<div class="mb-3 border-b border-gray-700 pb-3">
					<h4 class="text-xs font-semibold text-blue-400 mb-2">Priority</h4>
					<div class="space-y-1.5">
						{#each [['critical', 'Critical'], ['high', 'High'], ['medium', 'Medium'], ['low', 'Low']] as [key, label]}
							<label class="flex items-center gap-2">
								<input type="color" bind:value={colors[key]} onchange={saveColors} class="w-6 h-5 rounded border border-gray-600 bg-transparent cursor-pointer" />
								<span class="text-xs text-gray-300">{label}</span>
								<span class="ml-auto text-[0.6rem] mono text-gray-600">{colors[key]}</span>
							</label>
						{/each}
					</div>
				</div>

				<div class="mb-3 border-b border-gray-700 pb-3">
					<h4 class="text-xs font-semibold text-blue-400 mb-2">Risk / Category</h4>
					<div class="space-y-1.5">
						{#each [['riskHigh', 'Risk: High'], ['riskMedium', 'Risk: Medium'], ['riskLow', 'Risk: Low'], ['bug', 'Bug'], ['feature', 'Feature'], ['docs', 'Docs']] as [key, label]}
							<label class="flex items-center gap-2">
								<input type="color" bind:value={colors[key]} onchange={saveColors} class="w-6 h-5 rounded border border-gray-600 bg-transparent cursor-pointer" />
								<span class="text-xs text-gray-300">{label}</span>
								<span class="ml-auto text-[0.6rem] mono text-gray-600">{colors[key]}</span>
							</label>
						{/each}
					</div>
				</div>

				<Button onclick={resetColors} color="alternative" size="xs">Reset defaults</Button>
			</Card>

			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<Heading tag="h3" class="text-base mb-4">Color Legend</Heading>
				<div class="grid grid-cols-2 gap-4 text-xs text-gray-300">
					<div>
						<h4 class="text-gray-500 mb-1 text-[0.6rem] uppercase">PR Status</h4>
						{#each [['noPr', 'No PR'], ['hasPr', 'PR open'], ['prReady', 'PR ready']] as [key, label]}
							<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded inline-block" style="background: {colors[key]}"></span> {label}</div>
						{/each}
					</div>
					<div>
						<h4 class="text-gray-500 mb-1 text-[0.6rem] uppercase">Priority</h4>
						{#each [['critical', 'Critical'], ['high', 'High'], ['medium', 'Medium'], ['low', 'Low']] as [key, label]}
							<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded inline-block" style="background: {colors[key]}"></span> {label}</div>
						{/each}
					</div>
					<div>
						<h4 class="text-gray-500 mb-1 text-[0.6rem] uppercase">Risk</h4>
						{#each [['riskHigh', 'High'], ['riskMedium', 'Medium'], ['riskLow', 'Low']] as [key, label]}
							<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded inline-block" style="background: {colors[key]}"></span> {label}</div>
						{/each}
					</div>
					<div>
						<h4 class="text-gray-500 mb-1 text-[0.6rem] uppercase">Category</h4>
						{#each [['bug', 'Bug'], ['feature', 'Feature'], ['docs', 'Docs']] as [key, label]}
							<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded inline-block" style="background: {colors[key]}"></span> {label}</div>
						{/each}
					</div>
				</div>
			</Card>
		</div>
	</TabItem>

	<!-- ========================= CONFIGURATION ========================= -->
	<TabItem title="Configuration">
		<Card class="bg-gray-800 border-gray-700 max-w-none">
			<Heading tag="h3" class="text-base mb-4">Configuration</Heading>
			<Helper class="mb-4">
				Read from <code class="rounded bg-gray-700 px-1 py-0.5">.wshm/config.toml</code>. Edit the file and restart the daemon to change.
			</Helper>

			<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
				{#each [
					['Triage', [['Enabled', 'true'], ['Auto-fix', 'false'], ['Confidence', '0.85']]],
					['PR Analysis', [['Enabled', 'true'], ['Auto-label', 'true'], ['Risk labels', 'true']]],
					['Merge Queue', [['Threshold', '15'], ['Strategy', 'rebase']]],
					['Sync', [['Interval', '5 min'], ['Full sync', '24h']]]
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
</Tabs>
