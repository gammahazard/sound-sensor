/**
 * Guardian — Service Worker (WASM build)
 * Enables offline access and "Add to Home Screen" full-screen launch.
 *
 * Strategy: network-first for app shell (picks up OTA updates immediately),
 * falls back to cache when offline. WebSocket traffic is never cached.
 */

const CACHE_NAME = 'guardian-wasm-v1';
const SHELL = [
    '/',
    '/index.html',
    '/guardian-pwa.js',
    '/guardian-pwa_bg.wasm',
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

    // Network-first: try network, fall back to cache (enables OTA updates)
    e.respondWith(
        fetch(e.request)
            .then(response => {
                // Update cache with fresh response
                if (response.ok && SHELL.includes(url.pathname)) {
                    const clone = response.clone();
                    caches.open(CACHE_NAME).then(c => c.put(e.request, clone));
                }
                return response;
            })
            .catch(() => {
                // Network failed — serve from cache (offline support)
                return caches.match(e.request).then(r =>
                    r || new Response('Offline', { status: 503, statusText: 'Service Unavailable' })
                );
            })
    );
});
