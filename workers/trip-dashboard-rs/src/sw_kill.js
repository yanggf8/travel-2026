// Kill switch served at /sw.js during rollback (plan Task 7 Step 4): it takes
// over from the live service worker, deletes every td-* cache, unregisters
// itself, and intercepts nothing. Deployed by changing what router.rs serves
// at /sw.js — no feature flag.
self.addEventListener('install', function () {
  self.skipWaiting();
});

self.addEventListener('activate', function (event) {
  event.waitUntil(
    (async function () {
      var names = await self.caches.keys();
      for (var i = 0; i < names.length; i++) {
        if (names[i].indexOf('td-')===0) await self.caches.delete(names[i]);
      }
      await self.registration.unregister();
    })()
  );
});
