<script lang="ts">
	import { goto } from '$app/navigation';
	import * as Dialog from '$lib/components/ui/dialog';
	import SearchIcon from '@lucide/svelte/icons/search';
	import CornerDownLeftIcon from '@lucide/svelte/icons/corner-down-left';
	import type { NavItem } from '$lib/nav';

	let {
		open = $bindable(false),
		items,
		recent = []
	}: { open: boolean; items: NavItem[]; recent?: string[] } = $props();

	let query = $state('');
	let selected = $state(0);
	let inputEl: HTMLInputElement | null = $state(null);

	type Entry = { key: string; label: string; hint: string; icon: NavItem['icon']; href: string };

	let entries: Entry[] = $derived.by(() => {
		const q = query.trim().toLowerCase();
		let matched = items.filter(
			(i) => !q || `${i.label} ${i.section} ${i.keywords ?? ''}`.toLowerCase().includes(q)
		);
		const recentSet = new Set(q ? [] : recent);
		if (!q) {
			// Empty query: recently visited pages first, like Linear's palette.
			const byHref = new Map(matched.map((i) => [i.href, i]));
			const first = recent.map((h) => byHref.get(h)).filter((i): i is NavItem => !!i);
			matched = [...first, ...matched.filter((i) => !recentSet.has(i.href))];
		}
		const pages = matched.map((i) => ({
			key: i.href,
			label: i.label,
			hint: recentSet.has(i.href)
				? 'Recent'
				: i.shortcut
					? `G ${i.shortcut.toUpperCase()}`
					: i.section === 'Main'
						? 'Page'
						: i.section,
			icon: i.icon,
			href: i.href
		}));
		// Always offer a full-text search so typing an issue title or number
		// works even when it matches no page name.
		const search = items.find((i) => i.href === '/search');
		if (q && search) {
			pages.push({
				key: 'search:' + q,
				label: `Search issues & PRs for “${query.trim()}”`,
				hint: 'Search',
				icon: search.icon,
				href: '/search?q=' + encodeURIComponent(query.trim())
			});
		}
		return pages;
	});

	$effect(() => {
		// Reset the highlight whenever the result list changes.
		entries;
		selected = 0;
	});

	$effect(() => {
		if (open) {
			query = '';
			queueMicrotask(() => inputEl?.focus());
		}
	});

	function choose(e: Entry | undefined) {
		if (!e) return;
		open = false;
		goto(e.href);
	}

	function onKeydown(ev: KeyboardEvent) {
		if (ev.key === 'ArrowDown') {
			ev.preventDefault();
			selected = Math.min(selected + 1, entries.length - 1);
		} else if (ev.key === 'ArrowUp') {
			ev.preventDefault();
			selected = Math.max(selected - 1, 0);
		} else if (ev.key === 'Enter') {
			ev.preventDefault();
			choose(entries[selected]);
		}
	}

	$effect(() => {
		document.getElementById(`cmd-item-${selected}`)?.scrollIntoView({ block: 'nearest' });
	});
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="top-[20%] translate-y-0 gap-0 overflow-hidden p-0 sm:max-w-lg" showCloseButton={false}>
		<Dialog.Title class="sr-only">Go to</Dialog.Title>
		<div class="flex items-center gap-2 border-b px-3">
			<SearchIcon class="size-4 shrink-0 text-muted-foreground" />
			<input
				bind:this={inputEl}
				bind:value={query}
				onkeydown={onKeydown}
				placeholder="Go to a page or search issues & PRs…"
				class="h-11 w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground"
				role="combobox"
				aria-expanded="true"
				aria-controls="cmd-list"
				aria-activedescendant={entries.length ? `cmd-item-${selected}` : undefined}
			/>
		</div>
		<ul id="cmd-list" role="listbox" class="max-h-80 overflow-y-auto p-1">
			{#each entries as e, idx (e.key)}
				{@const Icon = e.icon}
				<!-- Keyboard selection is handled by the combobox input above. -->
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<li
					id="cmd-item-{idx}"
					role="option"
					aria-selected={idx === selected}
					class="flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-sm {idx === selected
						? 'bg-accent text-accent-foreground'
						: 'text-foreground/80'}"
					onmousemove={() => (selected = idx)}
					onclick={() => choose(e)}
				>
					<Icon class="size-4 shrink-0 text-muted-foreground" />
					<span class="flex-1 truncate">{e.label}</span>
					{#if idx === selected}
						<CornerDownLeftIcon class="size-3.5 text-muted-foreground" />
					{:else}
						<span class="text-xs text-muted-foreground">{e.hint}</span>
					{/if}
				</li>
			{:else}
				<li class="px-2 py-6 text-center text-sm text-muted-foreground">No matching page.</li>
			{/each}
		</ul>
		<div class="flex items-center gap-3 border-t px-3 py-1.5 text-[0.7rem] text-muted-foreground">
			<span><kbd class="kbd">↑</kbd> <kbd class="kbd">↓</kbd> to move</span>
			<span><kbd class="kbd">↵</kbd> to open</span>
			<span><kbd class="kbd">esc</kbd> to close</span>
			<span class="ml-auto"><kbd class="kbd">G</kbd> then a letter to jump</span>
		</div>
	</Dialog.Content>
</Dialog.Root>

<style>
	.kbd {
		border: 1px solid var(--border);
		border-radius: 4px;
		padding: 0 4px;
		font-family: inherit;
		font-size: 0.65rem;
	}
</style>
