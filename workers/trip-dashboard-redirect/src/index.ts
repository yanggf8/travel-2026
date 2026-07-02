// Minimal redirect worker: forwards the retired trip-dashboard.yanggf.workers.dev URL to the
// live -rs worker, preserving path + query so old share links (?plan=…&token=…) keep working.
//
// The legacy TS dashboard app was undeployed 2026-07-02 (see docs/plans/2026-07-02-deploy-day-
// checklist.md); this reclaims the same worker name purely to 301 old bookmarks to the new URL.
// No secrets, no Turso, no OAuth — just a redirect.

const TARGET_HOST = "trip-dashboard-rs.yanggf.workers.dev";

export default {
  fetch(request: Request): Response {
    const url = new URL(request.url);
    url.hostname = TARGET_HOST;
    // 301 permanent — this is the intended end-state (old URL retired, -rs is canonical).
    return Response.redirect(url.toString(), 301);
  },
};
