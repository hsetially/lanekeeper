# Worked examples

## Contents

1. Config-server fixture: property view and rendering
2. Channel override
3. Path mapping cases
4. Drift walk-through
5. Real-data expectations (sit1 and base)

## 1. Config-server fixture: property view and rendering

These come from the config-server repo's tests (`tx-config-client/src/test/resources/test-config`).

`config/application.properties`:
```
CREATE_VIRTUAL_ITEM = true
CORE_ROUTING = false
LOG_LEVEL: INFO
```
`config/application-bis.properties`:
```
CREATE_VIRTUAL_ITEM = false
CORE_ROUTING = true
LOG_LEVEL: DEBUG
```
The property view for tenant `bis` is `CREATE_VIRTUAL_ITEM=false`, `CORE_ROUTING=true`, `LOG_LEVEL=DEBUG`. The tenant file wins, key by key.

The resource `config/tx-infinity-core.yml` contains `coreRouting: ${CORE_ROUTING}` and `logLevel: ${LOG_LEVEL}`. It renders as:

- for tenant `bis`: `coreRouting: true`, `logLevel: DEBUG` (expected file `tx-infinity-core-resolved-bis.yml`);
- with no tenant: `coreRouting: false`, `logLevel: INFO` (`tx-infinity-core-resolved.yml`).

## 2. Channel override

`config/entryGroupConfig.yml` is the base. `config/remoteteller/entryGroupConfig-bis.yml` is the tenant `bis` copy in channel `remoteteller`.

A request for tenant `bis` with channel `remoteteller` gets the channel folder's tenant file, chosen whole and rendered. Every `${CREATE_VIRTUAL_ITEM}` becomes `false` (expected file `entryGroupConfig-resolved-bis.yml`).

## 3. Path mapping cases

| Repo path, branch | NFS path |
|---|---|
| `data/config/tx-infinity-api/tx-infinity-core.yml`, sit1 | `tx-infinity-api/tx-infinity-core-sit1.yml` |
| `data/config/ui/remote-itm-teller/ui-common-config.yaml`, sit7 | `ui/remote-itm-teller/ui-common-config-sit7.yaml` |
| `config/limits/limit-profiles.yml`, base | `limits/limit-profiles.yml` |
| `data/config/jwt-proxy-injector-service/mappingsItemEvaluationE2ETest`, sit1 | `jwt-proxy-injector-service/mappingsItemEvaluationE2ETest-sit1` |

Reverse-mapping `tx-infinity-core-sit1.yml` gives tenant `sit1` and name `tx-infinity-core.yml`. It never gives `tx`.

## 4. Drift walk-through

1. **Adoption.** It sets B = N = G for `limits/limit-profiles-sit7.yml`. State: `in_sync`.
2. **Someone edits the file over IAP.** N changes, while G = B. State: `nfs_ahead`. The sentinel attributes the edit to `lkowalski` with high confidence.
3. **A PR merges to sit7 with different content.** G changes too. State: `conflict`.
4. **The sync Job copies sit7.** N becomes equal to G. State: `in_sync`, with B = N. Because the earlier `nfs_ahead` content was overwritten, check C7 fires and links the lost version.

## 5. Real-data expectations (sit1 and base)

- **C2:** `tx-infinity-api/account-sorting-config.yml`, `tx-infinity-api/miniStatementConfig.yml`, `document-service/resources/receipt-ci_1.bmp`.
- **C1:** `tx-infinity-api/entryGroupConfig.yml`. The sit1 copy has no `atm` key under `entryGroupConfigMap`.
- **C8:** `security/security-roles.yml` repeats `txRemoteJuniorTeller`, `txRemoteSeniorTeller` and `txRemoteSupervisor`.
- **C9 (base):** 13 names, including `authentication-config.yaml` (12 folders), `error-codes-config.yaml` (4), `hold-details.ej.yaml` and `transaction.ej.yaml`.
- **Channels in `channels.yml`:** `remote-itm-teller`, `atm-iso`, `atm`.
- **Channel folders:**
  - `ui/remote-itm-teller`;
  - `tx-infinity-api/remote-itm-teller`;
  - `holds/remote-itm-teller`;
  - `holds/atm-iso`.
