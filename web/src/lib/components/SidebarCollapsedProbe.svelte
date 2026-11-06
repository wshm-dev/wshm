<script lang="ts">
	import { useSidebar } from '$lib/components/ui/sidebar/context.svelte.js';

	// The layout renders <Sidebar.Provider> itself, so it cannot read the
	// sidebar context; this child reports the desktop icon-only state up.
	let { collapsed = $bindable(false) }: { collapsed: boolean } = $props();
	const sidebar = useSidebar();
	$effect(() => {
		collapsed = sidebar.state === 'collapsed' && !sidebar.isMobile;
	});
</script>
