// What the hub offers for a restart, shared by the scenarios of the Services tab (./index.ts) and of the file page (./file.ts).
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type` for every type.
import type { ConfigServerRestartOfferResult, RestartOffer } from "../../../api/generated/model/index.ts";

export const CONFIG_SERVER = "csp-configuration-server";

/** The services that cannot be served while the config-server restarts. */
export const OFFER_SERVICES = [
  { namespace: "sitb", name: "tx-infinity-core" },
  { namespace: "sitb", name: "ui-service" },
  { namespace: "sitb", name: "document-service" },
  { namespace: "sitb", name: "holds-service" },
];

/** The offer for a swimlane that has check C11 open and exactly one config-server Deployment. */
export const CONFIG_SERVER_OFFER: ConfigServerRestartOfferResult = { offer: { service: { namespace: "sitb", name: CONFIG_SERVER }, affected: OFFER_SERVICES, truncated: false } };
export const NO_OFFER: ConfigServerRestartOfferResult = { offer: null };

/** The restart-all offer for a file in an application's folder: that application's Deployments. */
export const RESTART_OFFER: RestartOffer = { services: [{ namespace: "sitb", name: "tx-infinity-api" }, { namespace: "sitb", name: "tx-infinity-core" }], truncated: false };
