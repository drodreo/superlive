// Component i18n registration (the shared-instance contract, architecture.md
// "Plane A"): a component registers its own namespace's resources into the
// HOST's i18next instance (the import-map singleton) at load time — all
// locales upfront, so language switches are instant with zero component
// cooperation. i18next stays external here: inside the shell the import map
// resolves the shared instance; standalone component development resolves
// its own (the registration call is then a no-op-grade local operation).
// Registered namespaces stay for the session (translation bundles are tiny;
// no unload cycle).

import i18next from "i18next";

/** A component namespace's resources: locale → key → text (nested objects allowed). */
export type ComponentI18nResources = Record<string, Record<string, unknown>>;

/**
 * Register a component namespace's resources (every locale, upfront) into
 * the shared i18next instance. Call once at component module load.
 */
export function registerI18n(
  namespace: string,
  resources: ComponentI18nResources,
): void {
  for (const [lng, bundle] of Object.entries(resources)) {
    i18next.addResourceBundle(lng, namespace, bundle, true, true);
  }
}
