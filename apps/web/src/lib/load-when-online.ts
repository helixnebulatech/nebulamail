// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Code loaded on demand (React.lazy, dynamic import): offline, a chunk the
// service worker hasn't kept yet can't load. Instead of failing (a lazy
// component that rejects reaches the route's error page), wait for the
// connection and try again; its Suspense fallback stays meanwhile.

export const loadWhenOnline = async <T>(load: () => Promise<T>): Promise<T> => {
  try {
    return await load();
  } catch {
    const online = Promise.withResolvers<undefined>();
    globalThis.addEventListener(
      "online",
      () => {
        online.resolve(undefined);
      },
      { once: true }
    );
    await online.promise;
    return await load();
  }
};
