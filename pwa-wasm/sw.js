/**
 * Guardian — Service Worker (WASM build)
 * Enables offline access and "Add to Home Screen" full-screen launch.
 *
 * File names are fixed (data-no-hash in Trunk.toml) so the SHELL list is stable.
 * Cache-first for app shell; WebSocket traffic is never cached.
 */

const CACHE_NAME = 'guardian-wasm-v1';  // bump when PWA assets change
const SHELL = [
    '/',
    '/index.html',
    '/guardian_pwa.js',
    '/guardian_pwa_bg.wasm',
    '/sw.js',
    '/manifest.json',
    '/icon-192.png',
    '/icon-512.png',
];

self.addEventListener('install', (e) => {
    e.waitUntil(
        caches.open(CACHE_NAME).then(c => c.addAll(SHELL))
    );
    self.skipWaiting();
});

self.addEventListener('activate', (e) => {
    e.waitUntil(
        caches.keys().then(keys =>
            Promise.all(keys.filter(k => k !== CACHE_NAME).map(k => caches.delete(k)))
        )
    );
    self.clients.claim();
});

self.addEventListener('fetch', (e) => {
    if (e.request.method !== 'GET') return;
    const url = new URL(e.request.url);
    if (url.protocol === 'ws:' || url.protocol === 'wss:') return;

    e.respondWith(
        caches.match(e.request).then(cached => {
            if (cached) return cached;
            return fetch(e.request).then(response => {
                if (SHELL.includes(url.pathname)) {
                    const clone = response.clone();
                    caches.open(CACHE_NAME).then(c => c.put(e.request, clone));
                }
                return response;
            });
        })
    );
});
