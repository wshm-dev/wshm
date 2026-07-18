<script lang="ts">
	import { marked } from 'marked';
	import DOMPurify from 'dompurify';

	let { source }: { source: string } = $props();

	// Issue/PR bodies come from third parties on the forge — render markdown
	// but sanitize the resulting HTML (DOMPurify) so scriptable payloads in a
	// crafted issue body can't execute in the dashboard.
	let html = $derived(
		DOMPurify.sanitize(marked.parse(source, { async: false, gfm: true, breaks: true }) as string)
	);
</script>

<!-- eslint-disable-next-line svelte/no-at-html-tags -- sanitized above -->
<div class="markdown-body text-sm text-gray-300 max-h-96 overflow-y-auto break-words">{@html html}</div>

<style>
	/* Minimal GFM-ish styling scoped to rendered bodies; the app has no
	   typography plugin, so style the handful of tags markdown emits. */
	.markdown-body :global(h1),
	.markdown-body :global(h2),
	.markdown-body :global(h3),
	.markdown-body :global(h4) {
		color: rgb(243 244 246);
		font-weight: 600;
		margin: 0.75rem 0 0.25rem;
		line-height: 1.3;
	}
	.markdown-body :global(h1) { font-size: 1.125rem; }
	.markdown-body :global(h2) { font-size: 1rem; }
	.markdown-body :global(h3),
	.markdown-body :global(h4) { font-size: 0.875rem; }
	.markdown-body :global(p) { margin: 0.375rem 0; }
	.markdown-body :global(ul),
	.markdown-body :global(ol) {
		margin: 0.375rem 0;
		padding-left: 1.25rem;
	}
	.markdown-body :global(ul) { list-style: disc; }
	.markdown-body :global(ol) { list-style: decimal; }
	.markdown-body :global(li) { margin: 0.125rem 0; }
	.markdown-body :global(code) {
		background: rgb(17 24 39);
		padding: 0.125rem 0.375rem;
		border-radius: 0.25rem;
		font-size: 0.8125rem;
	}
	.markdown-body :global(pre) {
		background: rgb(17 24 39);
		padding: 0.625rem 0.75rem;
		border-radius: 0.375rem;
		overflow-x: auto;
		margin: 0.5rem 0;
	}
	.markdown-body :global(pre code) {
		background: transparent;
		padding: 0;
	}
	.markdown-body :global(a) {
		color: rgb(96 165 250);
		text-decoration: underline;
	}
	.markdown-body :global(blockquote) {
		border-left: 3px solid rgb(75 85 99);
		padding-left: 0.75rem;
		color: rgb(156 163 175);
		margin: 0.5rem 0;
	}
	.markdown-body :global(table) {
		border-collapse: collapse;
		margin: 0.5rem 0;
		font-size: 0.8125rem;
	}
	.markdown-body :global(th),
	.markdown-body :global(td) {
		border: 1px solid rgb(55 65 81);
		padding: 0.25rem 0.5rem;
	}
	.markdown-body :global(img) { max-width: 100%; }
	.markdown-body :global(hr) {
		border-color: rgb(55 65 81);
		margin: 0.75rem 0;
	}
</style>
