<script lang="ts">
	import { Card, Badge, Button, Input, Label, Helper, Alert, Heading } from 'flowbite-svelte';

	type Status = 'configured' | 'not_configured' | 'unknown';

	interface Props {
		title: string;
		// Visual badge: green=configured, gray=not configured, blue for hint label
		status: Status;
		statusLabel?: string;
		// Help text (HTML allowed via {@html})
		helpHtml?: string;
		// Token input
		tokenLabel?: string;
		tokenPlaceholder?: string;
		// Optional URL input (for self-host: Ollama, Gitea, Forgejo, GitLab self-host)
		urlLabel?: string;
		urlPlaceholder?: string;
		urlValue?: string;
		// Callbacks
		onSave: (payload: { token: string; url?: string }) => Promise<void>;
		onTest?: () => Promise<{ ok: boolean; message: string }>;
	}

	let {
		title,
		status,
		statusLabel,
		helpHtml = '',
		tokenLabel = 'Token',
		tokenPlaceholder = '',
		urlLabel,
		urlPlaceholder = '',
		urlValue = $bindable(''),
		onSave,
		onTest,
	}: Props = $props();

	let token: string = $state('');
	let saving: boolean = $state(false);
	let testing: boolean = $state(false);
	let saveMessage: string | null = $state(null);
	let saveError: boolean = $state(false);
	let testMessage: string | null = $state(null);
	let testError: boolean = $state(false);

	const badgeColor = status === 'configured' ? 'green' : 'dark';
	const badgeText = statusLabel ?? (status === 'configured' ? 'Configured' : 'Not configured');

	async function handleSave() {
		if (!token.trim()) return;
		saving = true; saveMessage = null; saveError = false;
		try {
			await onSave({ token: token.trim(), url: urlValue?.trim() || undefined });
			saveMessage = 'Saved.';
			token = '';
		} catch (e) {
			saveMessage = e instanceof Error ? e.message : 'Save failed';
			saveError = true;
		}
		saving = false;
	}

	async function handleTest() {
		if (!onTest) return;
		testing = true; testMessage = null; testError = false;
		try {
			const r = await onTest();
			testMessage = r.message;
			testError = !r.ok;
		} catch (e) {
			testMessage = e instanceof Error ? e.message : 'Test failed';
			testError = true;
		}
		testing = false;
	}
</script>

<Card class="bg-gray-800 border-gray-700 max-w-none">
	<Heading tag="h3" class="text-base mb-4">{title}</Heading>

	<div class="mb-3">
		<Badge large color={badgeColor}>{badgeText}</Badge>
	</div>

	{#if helpHtml}
		<Helper class="mb-3">{@html helpHtml}</Helper>
	{/if}

	{#if saveMessage}
		<Alert color={saveError ? 'red' : 'green'} class="text-xs py-2 mb-2">{saveMessage}</Alert>
	{/if}
	{#if testMessage}
		<Alert color={testError ? 'red' : 'green'} class="text-xs py-2 mb-2">{testMessage}</Alert>
	{/if}

	<form onsubmit={(e) => { e.preventDefault(); handleSave(); }} class="space-y-2">
		{#if urlLabel}
			<div>
				<Label class="text-xs mb-1">{urlLabel}</Label>
				<Input type="text" bind:value={urlValue} placeholder={urlPlaceholder} disabled={saving} size="sm" />
			</div>
		{/if}
		<div>
			<Label class="text-xs mb-1">{tokenLabel}</Label>
			<Input type="password" bind:value={token} placeholder={tokenPlaceholder} disabled={saving} size="sm" />
		</div>
		<div class="flex gap-2">
			<Button type="submit" color="blue" disabled={saving || !token.trim()} size="sm" class="flex-1">
				{saving ? 'Saving...' : 'Save'}
			</Button>
			{#if onTest}
				<Button type="button" color="alternative" onclick={handleTest} disabled={testing} size="sm" class="flex-1">
					{testing ? 'Testing...' : 'Test'}
				</Button>
			{/if}
		</div>
	</form>
</Card>
