<script lang="ts">
	let username = $state('');
	let password = $state('');
	let submitting = $state(false);
	let error = $state<string | null>(null);

	async function handleSubmit(event: Event) {
		event.preventDefault();
		submitting = true;
		error = null;
		try {
			const res = await fetch('/api/v1/auth/login', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ username, password })
			});
			if (!res.ok) {
				const body = await res.json().catch(() => ({}));
				error = body.error ?? `Login failed (${res.status})`;
				submitting = false;
				return;
			}
			window.location.replace('/');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Login failed';
			submitting = false;
		}
	}
</script>

<svelte:head>
	<title>Sign in to wshm</title>
</svelte:head>

<div class="min-h-screen flex items-center justify-center bg-gray-900 text-gray-200 px-4 py-12">
	<div class="w-full max-w-sm">
		<div class="flex flex-col items-center mb-8">
			<img src="/wizard-icon.png" alt="" class="h-14 w-14 mb-3" />
			<h1 class="text-2xl font-semibold text-gray-100">Welcome to wshm</h1>
			<p class="text-sm text-gray-500 mt-1">Sign in to continue</p>
		</div>

		<form onsubmit={handleSubmit} class="space-y-4">
			<div>
				<label for="username" class="block text-xs uppercase tracking-wider text-gray-400 mb-1.5">
					Username
				</label>
				<input
					id="username"
					type="text"
					autocomplete="username"
					bind:value={username}
					required
					class="w-full rounded-md border border-gray-700 bg-gray-800 px-3 py-2 text-sm text-gray-100 placeholder-gray-500 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
					placeholder="admin"
				/>
			</div>

			<div>
				<label for="password" class="block text-xs uppercase tracking-wider text-gray-400 mb-1.5">
					Password
				</label>
				<input
					id="password"
					type="password"
					autocomplete="current-password"
					bind:value={password}
					required
					class="w-full rounded-md border border-gray-700 bg-gray-800 px-3 py-2 text-sm text-gray-100 placeholder-gray-500 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
				/>
			</div>

			{#if error}
				<div class="rounded-md border border-red-700 bg-red-900/30 px-3 py-2 text-xs text-red-300">
					{error}
				</div>
			{/if}

			<button
				type="submit"
				disabled={submitting}
				class="w-full rounded-md bg-blue-600 hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed text-white text-sm font-semibold px-4 py-2.5 transition"
			>
				{submitting ? 'Signing in…' : 'Log in'}
			</button>
		</form>

		<div class="flex items-center my-6">
			<div class="flex-1 h-px bg-gray-700"></div>
			<span class="px-3 text-xs uppercase tracking-wider text-gray-500">or</span>
			<div class="flex-1 h-px bg-gray-700"></div>
		</div>

		<a
			href="/oauth2/start?rd=/"
			class="flex items-center justify-center gap-3 w-full rounded-md border border-gray-700 bg-gray-800 hover:bg-gray-700 text-sm font-medium text-gray-200 px-4 py-2.5 transition"
		>
			<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" class="h-5 w-5" aria-hidden="true">
				<path fill="#4285F4" d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z"/>
				<path fill="#34A853" d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84A11 11 0 0 0 12 23z"/>
				<path fill="#FBBC05" d="M5.84 14.09a6.6 6.6 0 0 1 0-4.18V7.07H2.18a11 11 0 0 0 0 9.86l3.66-2.84z"/>
				<path fill="#EA4335" d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1A11 11 0 0 0 2.18 7.07l3.66 2.84C6.71 7.31 9.14 5.38 12 5.38z"/>
			</svg>
			<span>Sign in with Google</span>
		</a>

		<p class="text-center text-xs text-gray-600 mt-8">
			wshm — open-source GitHub agent
		</p>
	</div>
</div>
