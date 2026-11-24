/**
 * Tombstone worker.
 *
 * An earlier build registered a service worker here for agent notifications.
 * That approach was dropped, but deleting the file does not remove a worker
 * already installed in a browser — it just makes the update check 404, leaving
 * the old worker running indefinitely. The only way to retire one is to ship a
 * replacement that unregisters itself.
 *
 * Safe to delete once no browser could still be holding the old registration.
 */

self.addEventListener("install", () => self.skipWaiting());

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      await self.registration.unregister();
      // Reload open tabs so they stop being controlled by a dead worker.
      const clients = await self.clients.matchAll({ type: "window" });
      for (const client of clients) client.navigate(client.url);
    })(),
  );
});
