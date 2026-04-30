import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [tailwindcss(), sveltekit()],
	server: {
		// Forward all backend calls to the running wshm daemon so the
		// SvelteKit dev server can proxy /api/* and /health without CORS.
		proxy: {
			'/api': {
				target: 'http://127.0.0.1:3002',
				changeOrigin: false,
				// Inject Basic Auth so we don't have to log in via the browser
				// every reload. Override via VITE_WSHM_BASIC if needed.
				configure: (proxy) => {
					proxy.on('proxyReq', (proxyReq) => {
						const creds = process.env.VITE_WSHM_BASIC
							?? Buffer.from('admin:test123').toString('base64');
						proxyReq.setHeader('Authorization', `Basic ${creds}`);
					});
				}
			},
			'/health': 'http://127.0.0.1:3002'
		}
	}
});
