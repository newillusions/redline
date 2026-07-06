/**
 * Session-only cache of passwords for password-protected PDFs, keyed by
 * absolute file path. NEVER persisted to disk (no localStorage/settings.json
 * write) - the Map lives only in the running app's memory and is gone on
 * restart. Lets reopening the same file within a session (a new tab, or the
 * same file after its tab was closed) skip re-prompting for the password.
 */

export type PasswordCache = Map<string, string>;

/** Create an empty session cache. One instance lives for the app's lifetime. */
export function createPasswordCache(): PasswordCache {
  return new Map();
}

/** Previously-entered password for `path`, if any. */
export function getCachedPassword(cache: PasswordCache, path: string): string | undefined {
  return cache.get(path);
}

/** Remember `password` as the working password for `path`. */
export function setCachedPassword(cache: PasswordCache, path: string, password: string): void {
  cache.set(path, password);
}

/** Forget the cached password for `path` (e.g. it turned out to be stale/wrong). */
export function clearCachedPassword(cache: PasswordCache, path: string): void {
  cache.delete(path);
}
