import { RepoKind } from "@/api/generated/model";

// The two repositories a GitHub token must be able to push to. The contract names them only as `base` and `tenant`
// (`TokenMissingAccessProblem`), and docs/domain-model.md fixes their names, so the instructions on the token screens use these.
// What the hub verified for the stored token (the `scopes` of `GithubCredential`) is shown as the hub sent it, not as this table says.
export const REPO_NAMES: Readonly<Record<RepoKind, string>> = { [RepoKind.base]: "configuration-base-saas", [RepoKind.tenant]: "csp-tenant-data" };

/** Where a person creates a fine-grained token on GitHub. A plain link: nothing is sent there from this app. */
export const GITHUB_TOKEN_SETTINGS_URL = "https://github.com/settings/personal-access-tokens/new";

/** The longest token the hub accepts (`GithubTokenRequest`). */
export const MAX_TOKEN_LENGTH = 512;
