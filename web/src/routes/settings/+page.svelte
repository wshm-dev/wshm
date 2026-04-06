<script lang="ts">
	import { onMount } from 'svelte';
	import { Card, Badge } from 'flowbite-svelte';
	import { colorConfig, type ColorConfig } from '$lib/colors';
	import { fetchLicense, type LicenseInfo } from '$lib/api';

	let colors: ColorConfig = $state({ ...colorConfig.defaults });
	colorConfig.subscribe(c => colors = { ...c });

	let license: LicenseInfo | null = $state(null);

	function save() {
		colorConfig.save(colors);
	}

	function reset() {
		colorConfig.reset();
		colors = { ...colorConfig.defaults };
	}

	onMount(async () => {
		try {
			license = await fetchLicense();
		} catch {
			// ignore
		}
	});
</script>

<svelte:head>
	<title>wshm - Settings</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-gray-100 mb-1">Settings</h2>
	<p class="text-sm text-gray-500">License, configuration, and display preferences</p>
</div>

<div class="grid grid-cols-1 lg:grid-cols-3 gap-4 mb-4">
	<!-- License -->
	<Card class="bg-gray-800 border-gray-700">
		<h3 class="text-base font-semibold text-gray-100 mb-4">License</h3>

		{#if license}
			<div class="flex items-center gap-3 mb-4">
				<Badge large color={license.is_pro ? 'green' : 'dark'}>
					{license.plan.toUpperCase()}
				</Badge>
				{#if !license.is_pro}
					<span class="text-sm text-gray-400">Free tier</span>
				{/if}
			</div>

			<div class="mb-4">
				<h4 class="text-sm font-semibold text-gray-400 mb-2">OSS Features (included)</h4>
				<div class="flex flex-wrap gap-1">
					{#each license.oss_features as f}
						<Badge color="blue">{f}</Badge>
					{/each}
				</div>
			</div>

			<div class="mb-4">
				<h4 class="text-sm font-semibold text-gray-400 mb-2">Pro Features</h4>
				<div class="space-y-1">
					{#each license.features as f}
						<div class="flex items-center justify-between text-sm">
							<span class="text-gray-300">{f.label}</span>
							{#if f.enabled}
								<Badge color="green">Active</Badge>
							{:else}
								<Badge color="dark">Locked</Badge>
							{/if}
						</div>
					{/each}
				</div>
			</div>

			{#if !license.is_pro}
				<div class="border-t border-gray-700 pt-3">
					<p class="text-sm text-gray-400 mb-2">Unlock all features with a Pro license:</p>
					<a href="https://wshm.dev/pro" target="_blank" class="inline-block rounded bg-blue-600 px-3 py-1.5 text-sm text-white hover:bg-blue-500">
						Get wshm Pro
					</a>
					<p class="text-xs text-gray-500 mt-2">
						Or run: <code class="rounded bg-gray-700 px-1 py-0.5">wshm login --license</code>
					</p>
				</div>
			{/if}
		{:else}
			<p class="text-sm text-gray-500">Loading...</p>
		{/if}
	</Card>

	<!-- Color Scheme -->
	<Card class="bg-gray-800 border-gray-700">
		<h3 class="text-base font-semibold text-gray-100 mb-4">Color Scheme</h3>

		<div class="mb-3 border-b border-gray-700 pb-3">
			<h4 class="text-xs font-semibold text-blue-400 mb-2">Issue PR Status</h4>
			<div class="space-y-1.5">
				{#each [['noPr', 'No PR'], ['hasPr', 'PR open'], ['prReady', 'PR ready']] as [key, label]}
					<label class="flex items-center gap-2">
						<input type="color" bind:value={colors[key]} onchange={save} class="w-6 h-5 rounded border border-gray-600 bg-transparent cursor-pointer" />
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
						<input type="color" bind:value={colors[key]} onchange={save} class="w-6 h-5 rounded border border-gray-600 bg-transparent cursor-pointer" />
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
						<input type="color" bind:value={colors[key]} onchange={save} class="w-6 h-5 rounded border border-gray-600 bg-transparent cursor-pointer" />
						<span class="text-xs text-gray-300">{label}</span>
						<span class="ml-auto text-[0.6rem] mono text-gray-600">{colors[key]}</span>
					</label>
				{/each}
			</div>
		</div>

		<button onclick={reset} class="rounded border border-gray-600 px-2 py-1 text-xs text-gray-400 hover:border-red-500 hover:text-red-400">
			Reset defaults
		</button>
	</Card>

	<!-- Configuration -->
	<Card class="bg-gray-800 border-gray-700">
		<h3 class="text-base font-semibold text-gray-100 mb-4">Configuration</h3>
		<p class="text-xs text-gray-500 mb-4">
			From <code class="rounded bg-gray-700 px-1 py-0.5">.wshm/config.toml</code>
		</p>

		{#each [
			['Triage', [['Enabled', 'true'], ['Auto-fix', 'false'], ['Confidence', '0.85']]],
			['PR Analysis', [['Enabled', 'true'], ['Auto-label', 'true'], ['Risk labels', 'true']]],
			['Merge Queue', [['Threshold', '15'], ['Strategy', 'rebase']]],
			['Sync', [['Interval', '5 min'], ['Full sync', '24h']]]
		] as [section, items]}
			<div class="mb-3 border-b border-gray-700 pb-3">
				<h4 class="text-xs font-semibold text-blue-400 mb-1">{section}</h4>
				<dl class="grid grid-cols-[120px_1fr] gap-x-2 gap-y-0.5">
					{#each items as [key, val]}
						<dt class="text-xs text-gray-500">{key}</dt>
						<dd class="text-xs text-gray-300 mono">{val}</dd>
					{/each}
				</dl>
			</div>
		{/each}
	</Card>
</div>

<!-- Legend -->
<Card class="bg-gray-800 border-gray-700">
	<h3 class="text-sm font-semibold text-gray-100 mb-2">Color Legend</h3>
	<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-xs text-gray-300">
		<div>
			<h4 class="text-gray-500 mb-1 text-[0.6rem] uppercase">PR Status</h4>
			{#each [['noPr', 'No PR'], ['hasPr', 'PR open'], ['prReady', 'PR ready']] as [key, label]}
				<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded" style="background: {colors[key]}"></span> {label}</div>
			{/each}
		</div>
		<div>
			<h4 class="text-gray-500 mb-1 text-[0.6rem] uppercase">Priority</h4>
			{#each [['critical', 'Critical'], ['high', 'High'], ['medium', 'Medium'], ['low', 'Low']] as [key, label]}
				<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded" style="background: {colors[key]}"></span> {label}</div>
			{/each}
		</div>
		<div>
			<h4 class="text-gray-500 mb-1 text-[0.6rem] uppercase">Risk</h4>
			{#each [['riskHigh', 'High'], ['riskMedium', 'Medium'], ['riskLow', 'Low']] as [key, label]}
				<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded" style="background: {colors[key]}"></span> {label}</div>
			{/each}
		</div>
		<div>
			<h4 class="text-gray-500 mb-1 text-[0.6rem] uppercase">Category</h4>
			{#each [['bug', 'Bug'], ['feature', 'Feature'], ['docs', 'Docs']] as [key, label]}
				<div class="flex items-center gap-1.5"><span class="w-2.5 h-2.5 rounded" style="background: {colors[key]}"></span> {label}</div>
			{/each}
		</div>
	</div>
</Card>
