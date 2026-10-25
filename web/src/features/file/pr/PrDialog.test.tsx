import { act, screen, waitFor, within } from "@testing-library/react";
import { Profiler } from "react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { FileEntry, PrJob, PrLink, PrPreview } from "@/api/generated/model";
import { HASH_A, json, problem, renderFile } from "../test-utils";
import { PrDialog, type PrDialogProps } from "./PrDialog";

// Raise a pull request (Consequence dialogs, 1a to 1k): choose the branch it goes to — the tenant's, or base — with the
// repository and the path it writes shown before anything is created, then a title and a description, then the job, which is the
// server's to report. A second PR for the same change is refused by the server and shown as a link, never created.

const PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
const entry = { path: PATH, logical_file: "tx-infinity-api/tx-infinity-core.yml", tenant: "sit7" } as unknown as FileEntry;
const baseEntry = { path: "tx-infinity-api/tx-infinity-core.yml", logical_file: "tx-infinity-api/tx-infinity-core.yml", tenant: null } as unknown as FileEntry;

const preview = (kind: "tenant" | "base", over: Partial<PrPreview> = {}): PrPreview => ({
  repo: kind === "tenant" ? "csp/csp-tenant-data" : "csp/configuration-base-saas",
  destination: kind === "tenant" ? { kind: "tenant", branch: "sit7" } : { kind: "base" },
  base_branch: kind === "tenant" ? "sit7" : "main",
  files: [{ nfs_path: PATH, repo_path: kind === "tenant" ? "data/config/tx-infinity-api/tx-infinity-core.yml" : "config/tx-infinity-api/tx-infinity-core.yml", exists_on_branch: true }],
  unchanged: [],
  warnings: [],
  open_pr: null,
  ...over,
});
const link: PrLink = { id: 3, swimlane: "sitb", path: PATH, nfs_hash: HASH_A, state: "open", pr_url: "https://github.com/csp/csp-tenant-data/pull/418", pr_number: 418, author: "u", created_at: 1, closed_at: null };
const job = (state: PrJob["state"], over: Partial<PrJob> = {}): PrJob => ({ id: 7, swimlane: "sitb", state, paths: [PATH], pr_url: null, error: null, created_at: 1, updated_at: 2, ...over });
const LANES = { items: [{ id: "sitb", display_name: "sitb", tenants: ["sit7"], agent: "connected", last_scan_at: null, base_version: null, drift_counts: [], finding_count: 0 }, { id: "sitd", display_name: "sitd", tenants: ["sit7", "sit8"], agent: "connected", last_scan_at: null, base_version: null, drift_counts: [], finding_count: 0 }], next: null };

type Server = { previews?: { tenant?: () => Response | PrPreview; base?: () => Response | PrPreview }; create?: () => Response; job?: () => Response };
const server = (s: Server = {}) => (call: { method: string; path: string; body: string | undefined }) => {
  if (call.path === "/api/v1/prs/preview") {
    const kind = (JSON.parse(call.body ?? "{}") as { target?: { kind: "tenant" | "base" } }).target?.kind ?? "tenant";
    const made = s.previews?.[kind]?.() ?? preview(kind);
    return made instanceof Response ? made : json(made);
  }
  if (call.path === "/api/v1/prs" && call.method === "POST") return s.create?.() ?? json(job("pending"), 202);
  if (call.path === "/api/v1/pr-jobs/7") return s.job?.() ?? json(job("completed", { pr_url: "https://github.com/csp/csp-tenant-data/pull/419" }));
  if (call.path.startsWith("/api/v1/swimlanes")) return json(LANES);
  return problem(404, "not_found");
};

const props: PrDialogProps = { open: true, onOpenChange: vi.fn(), lane: "sitb", path: PATH, entry, tenants: ["sit7"], hash: HASH_A, onSettled: vi.fn(), onTokenExpired: vi.fn() };

async function toForm(target: "tenant" | "base" = "tenant") {
  if (target === "base") await userEvent.click(await screen.findByRole("radio", { name: /^Base/ }));
  await userEvent.click(await screen.findByRole("button", { name: "Continue" }));
  return screen.findByLabelText("Title");
}

describe("PrDialog: choosing the target", () => {
  it("shows the repository, the branch and the path each target writes, from the server, before anything is created (1b)", async () => {
    const { calls } = renderFile(<PrDialog {...props} />, { api: server() });
    const dialog = await screen.findByRole("dialog", { name: "Raise pull request" });
    const tenant = await within(dialog).findByRole("radio", { name: "Tenant branch sit7" });
    expect(tenant).toBeChecked();
    expect(await within(dialog).findByText("csp/csp-tenant-data")).toBeInTheDocument();
    expect(within(dialog).getByText("data/config/tx-infinity-api/tx-infinity-core.yml")).toBeInTheDocument();
    expect(within(dialog).getByText("csp/configuration-base-saas")).toBeInTheDocument();
    expect(within(dialog).getByText("config/tx-infinity-api/tx-infinity-core.yml")).toBeInTheDocument();
    expect(within(dialog).getByText("Step 1 of 2")).toBeInTheDocument();
    const previews = calls.filter((c) => c.path === "/api/v1/prs/preview").map((c) => JSON.parse(c.body ?? "{}") as { swimlane: string; paths: string[]; target: object });
    expect(previews.map((p) => p.target)).toEqual(expect.arrayContaining([{ kind: "tenant", branch: "sit7" }, { kind: "base" }]));
    expect(previews[0]).toMatchObject({ swimlane: "sitb", paths: [PATH] });
    expect(calls.filter((c) => c.path === "/api/v1/prs" && c.method === "POST")).toHaveLength(0);
  });

  it("draws a target that is still being checked as a named status region (axe: `aria-label` needs a role)", async () => {
    // The previews are held, so the dialog is looked at while it is still checking where each target goes.
    renderFile(<PrDialog {...props} />, { api: (call) => (call.path === "/api/v1/prs/preview" ? new Promise<Response>(() => undefined) : server()(call)) });
    const dialog = await screen.findByRole("dialog", { name: "Raise pull request" });
    const checking = await within(dialog).findAllByRole("status", { name: "Checking where this goes" });
    expect(checking.length).toBeGreaterThan(0);
    for (const region of checking) expect(region).toHaveAttribute("aria-busy", "true");
  });

  it("names the swimlanes that run the tenant branch (1b)", async () => {
    renderFile(<PrDialog {...props} />, { api: server() });
    expect(await screen.findByText(/Affects swimlanes running sit7:/)).toHaveTextContent("Affects swimlanes running sit7: sitb, sitd");
  });

  it("says a change to base reaches every swimlane, and warns that it copies a tenant's content into base (1c)", async () => {
    renderFile(<PrDialog {...props} />, { api: server() });
    await userEvent.click(await screen.findByRole("radio", { name: /^Base/ }));
    expect(await screen.findByText("Every swimlane gets this change on its next sync.")).toBeInTheDocument();
    expect(screen.getByText("This copies one tenant's content into base and affects every swimlane on its next sync.")).toBeInTheDocument();
  });

  it("says a tenant change reaches this branch only (1b)", async () => {
    renderFile(<PrDialog {...props} />, { api: server() });
    expect(await screen.findByText("This PR changes sit7 only.")).toBeInTheDocument();
    expect(screen.getByText("sitb picks it up after the PR merges and the swimlane syncs.")).toBeInTheDocument();
  });

  it("warns that a file new on the branch forks from base (1d)", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ previews: { tenant: () => preview("tenant", { files: [{ nfs_path: PATH, repo_path: "data/config/tx-infinity-api/tx-infinity-core.yml", exists_on_branch: false }] }) } }) });
    expect(await screen.findByText("This PR adds a new file on sit7 and forks it from base.")).toBeInTheDocument();
    expect(screen.getByText("New file on sit7. Future base changes won't reach sit7 for this file.")).toBeInTheDocument();
  });

  it("shows the server's own warnings for a target", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ previews: { base: () => preview("base", { warnings: ["A tenant file is copied into base."] }) } }) });
    expect(await screen.findByText("A tenant file is copied into base.")).toBeInTheDocument();
  });

  it("starts on base for a base file, which has no tenant branch of its own", async () => {
    renderFile(<PrDialog {...props} entry={baseEntry} tenants={["sit7"]} path="tx-infinity-api/tx-infinity-core.yml" />, { api: server() });
    expect(await screen.findByRole("radio", { name: /^Base/ })).toBeChecked();
  });

  it("offers the tenant branches of the swimlane for a base file, and previews the one chosen", async () => {
    const { calls } = renderFile(<PrDialog {...props} entry={baseEntry} tenants={["sit7", "sit8"]} />, { api: server() });
    await userEvent.selectOptions(await screen.findByLabelText("Tenant branch"), "sit8");
    await waitFor(() => expect(calls.some((c) => c.path === "/api/v1/prs/preview" && c.body?.includes('"branch":"sit8"') === true)).toBe(true));
  });

  it("has only base when the swimlane has no tenant yet", async () => {
    renderFile(<PrDialog {...props} entry={baseEntry} tenants={[]} />, { api: server() });
    await screen.findByRole("radio", { name: /^Base/ });
    expect(screen.queryByRole("radio", { name: /^Tenant branch/ })).not.toBeInTheDocument();
  });

  it("says when a target cannot be previewed, and lets the other one be chosen", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ previews: { tenant: () => problem(422, "unprocessable", { reason: "invalid_content" }) } }) });
    expect(await screen.findByText(/This target could not be checked/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("radio", { name: /^Base/ }));
    expect(screen.getByRole("button", { name: "Continue" })).not.toHaveAttribute("aria-disabled", "true");
  });

  it("will not go on while the chosen target has not been checked", async () => {
    renderFile(<PrDialog {...props} />, { api: (call) => (call.path === "/api/v1/prs/preview" ? new Promise<Response>(() => undefined) : server()(call)) });
    expect(await screen.findByRole("button", { name: "Continue" })).toHaveAttribute("aria-disabled", "true");
  });

  it("says there is nothing to raise when the branch already has this content", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ previews: { tenant: () => preview("tenant", { files: [], unchanged: [PATH] }) } }) });
    expect(await screen.findByText(/The branch already has this content/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Continue" })).toHaveAttribute("aria-disabled", "true");
  });

  it("moves between the targets with the arrow keys", async () => {
    renderFile(<PrDialog {...props} />, { api: server() });
    const tenant = await screen.findByRole("radio", { name: "Tenant branch sit7" });
    tenant.focus();
    // Radix checks a radio when focus arrives while an arrow key is still held, so hold the key across the focus move.
    await userEvent.keyboard("{ArrowRight>}");
    await waitFor(() => expect(screen.getByRole("radio", { name: /^Base/ })).toHaveFocus());
    await userEvent.keyboard("{/ArrowRight}");
    expect(screen.getByRole("radio", { name: /^Base/ })).toBeChecked();
  });
});

describe("PrDialog: the open PR that already exists (1f)", () => {
  it("shows the PR as a link and offers no create when the preview names one", async () => {
    userEvent.setup();
    renderFile(<PrDialog {...props} />, { api: server({ previews: { tenant: () => preview("tenant", { open_pr: link }) } }) });
    await userEvent.click(await screen.findByRole("button", { name: "Continue" }));
    expect(await screen.findByText("PR #418 is already open for this change.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open PR #418" })).toHaveAttribute("href", "https://github.com/csp/csp-tenant-data/pull/418");
    expect(screen.queryByRole("button", { name: "Create PR" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Copy PR link" }));
    expect(await navigator.clipboard.readText()).toBe("https://github.com/csp/csp-tenant-data/pull/418");
  });

  it("shows the same when the server refuses the second PR with duplicate_pr", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ create: () => problem(409, "conflict", { reason: "duplicate_pr", link }) }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    expect(await screen.findByText("PR #418 is already open for this change.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open PR #418" })).toBeInTheDocument();
  });

  it("draws the open PR as a card with the file and who opened it, says what is the same, and links the swimlane's pull requests (the shared duplicate-PR view)", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ create: () => problem(409, "conflict", { reason: "duplicate_pr", link }) }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    const card = (await screen.findByText("Pull request #418")).closest<HTMLElement>("[data-slot='pr-card']");
    expect(card).not.toBeNull();
    expect(within(card as HTMLElement).getByText(PATH)).toBeInTheDocument();
    expect(screen.getByText(/Same target and same content \(NFS hash aaaaaaaaaaaa\)/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "All pull requests for sitb" })).toHaveAttribute("href", "/swimlanes/sitb/pull-requests");
  });

  it("does not take a malformed link in the 409 for an open pull request: it is an ordinary conflict", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ create: () => problem(409, "conflict", { reason: "duplicate_pr", link: { ...link, pr_number: "x" } }) }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    await waitFor(() => expect(screen.queryByText("PR #418 is already open for this change.")).not.toBeInTheDocument());
    expect(screen.queryByRole("link", { name: /Open PR/ })).not.toBeInTheDocument();
  });

  it("makes no link of a PR URL that is not https", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ previews: { tenant: () => preview("tenant", { open_pr: { ...link, pr_url: "javascript:alert(1)" } }) } }) });
    await userEvent.click(await screen.findByRole("button", { name: "Continue" }));
    await screen.findByText("PR #418 is already open for this change.");
    expect(screen.queryByRole("link", { name: /Open PR/ })).not.toBeInTheDocument();
  });
});

describe("PrDialog: the title and the description (1e)", () => {
  it("starts with a title and a description made from the swimlane, the file, the target and the hash", async () => {
    renderFile(<PrDialog {...props} />, { api: server() });
    const title = await toForm();
    expect(title).toHaveValue("Update tx-infinity-core for sit7 (from sitb)");
    const description = screen.getByLabelText<HTMLTextAreaElement>("Description").value;
    expect(description).toContain("Swimlane: sitb (csp/csp-tenant-data@sit7)");
    expect(description).toContain(`NFS hash: ${HASH_A.slice(0, 12)}`);
    expect(screen.getByText("Step 2 of 2")).toBeInTheDocument();
    expect(screen.getByText("data/config/tx-infinity-api/tx-infinity-core.yml")).toBeInTheDocument();
  });

  it("goes back to the targets, keeping the choice", async () => {
    renderFile(<PrDialog {...props} />, { api: server() });
    await toForm("base");
    await userEvent.click(screen.getByRole("button", { name: "Change" }));
    expect(await screen.findByRole("radio", { name: /^Base/ })).toBeChecked();
  });

  it("needs a title", async () => {
    renderFile(<PrDialog {...props} />, { api: server() });
    const title = await toForm();
    await userEvent.clear(title);
    expect(screen.getByRole("button", { name: "Create PR" })).toHaveAttribute("aria-disabled", "true");
  });

  it("sends the choice, the title and the description", async () => {
    const { calls } = renderFile(<PrDialog {...props} />, { api: server() });
    const title = await toForm();
    await userEvent.clear(title);
    await userEvent.type(title, "My change");
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/api/v1/prs" && c.method === "POST")).toBe(true));
    const sent = JSON.parse(calls.find((c) => c.path === "/api/v1/prs" && c.method === "POST")?.body ?? "{}") as Record<string, unknown>;
    expect(sent).toMatchObject({ swimlane: "sitb", paths: [PATH], title: "My change", target: { kind: "tenant", branch: "sit7" } });
    expect(String(sent["body"])).toContain("Raised from Lanekeeper on swimlane sitb.");
  });

  it("sends the base target when base was chosen", async () => {
    const { calls } = renderFile(<PrDialog {...props} />, { api: server() });
    await toForm("base");
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/api/v1/prs" && c.method === "POST")).toBe(true));
    expect(JSON.parse(calls.find((c) => c.path === "/api/v1/prs" && c.method === "POST")?.body ?? "{}")).toMatchObject({ target: { kind: "base" } });
  });
});

describe("PrDialog: the job (1g to 1i)", () => {
  it("follows the job the server reports, step by step, and links the PR when it is done", async () => {
    userEvent.setup();
    renderFile(<PrDialog {...props} />, { api: server({ create: () => json(job("committing"), 202), job: () => json(job("completed", { pr_url: "https://github.com/csp/csp-tenant-data/pull/419" })) }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    expect(await screen.findByText("Pull request created")).toBeInTheDocument();
    expect(await screen.findByText("PR #419 created.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open PR #419 on GitHub" })).toHaveAttribute("href", "https://github.com/csp/csp-tenant-data/pull/419");
    expect(screen.getByText("The file stays NFS ahead until the PR merges and sitb syncs.")).toBeInTheDocument();
    // The address of the PR can be copied, as the server gave it (it is an https address, checked before it is a link).
    await userEvent.click(screen.getByRole("button", { name: "Copy PR link" }));
    expect(await navigator.clipboard.readText()).toBe("https://github.com/csp/csp-tenant-data/pull/419");
  });

  it("shows the steps while the job runs, and says the dialog can be closed without stopping it", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ create: () => json(job("committing"), 202), job: () => json(job("committing")) }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    expect(await screen.findByText("Creating your pull request.")).toBeInTheDocument();
    const steps = screen.getAllByRole("listitem");
    expect(steps.map((s) => s.textContent)).toEqual(expect.arrayContaining([expect.stringContaining("Preparing"), expect.stringContaining("Committing"), expect.stringContaining("Opening pull request"), expect.stringContaining("Done")]));
    expect(screen.getByText("Preparing").closest("li")).toHaveAttribute("data-status", "done");
    expect(screen.getByText("Committing").closest("li")).toHaveAttribute("data-status", "active");
    expect(screen.getByText(/Esc closes this dialog, not the job/)).toBeInTheDocument();
  });

  it("says a queued job has not started, and marks no step as being worked on", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ create: () => json(job("pending"), 202), job: () => json(job("pending")) }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    expect(await screen.findByText("Waiting to start.")).toBeInTheDocument();
    expect(screen.getByText("Preparing").closest("li")).toHaveAttribute("data-status", "waiting");
    expect(document.querySelector("li[data-status='active']")).toBeNull();
  });

  it("points to the swimlane's pull requests while the job runs, since the job's id is not kept across a reload", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ create: () => json(job("committing"), 202), job: () => json(job("committing")) }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    expect(await screen.findByRole("link", { name: "Find my pull request" })).toHaveAttribute("href", "/swimlanes/sitb/pull-requests");
    expect(screen.queryByText(/Resumed job/)).not.toBeInTheDocument();
  });

  it("says what the server reported when the job is completed without an address that can be opened, and never links a PR it cannot name", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ create: () => json(job("opening"), 202), job: () => json(job("completed", { pr_url: "javascript:alert(1)" })) }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    expect(await screen.findByText("The job finished, but the server gave no address that can be opened.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /on GitHub/ })).not.toBeInTheDocument();
  });

  it("says a job failed with what the server said, and never says it was created", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ create: () => json(job("pending"), 202), job: () => json(job("failed", { error: "push rejected by the branch rules" })) }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    expect(await screen.findByText("Pull request not created")).toBeInTheDocument();
    expect(screen.getByText("push rejected by the branch rules")).toBeInTheDocument();
    expect(screen.queryByText(/created\./)).not.toBeInTheDocument();
  });

  it("shows the warnings the job carries", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ create: () => json(job("opening", { warnings: ["A new tenant file forks from base."] }), 202), job: () => json(job("opening", { warnings: ["A new tenant file forks from base."] })) }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    expect(await screen.findByText("A new tenant file forks from base.")).toBeInTheDocument();
  });
});

describe("PrDialog: the answers that are not a job", () => {
  it("hands a request sent for approval to the page, and closes", async () => {
    const onSettled = vi.fn();
    const onOpenChange = vi.fn();
    renderFile(<PrDialog {...props} onSettled={onSettled} onOpenChange={onOpenChange} />, { api: server({ create: () => json({ outcome: "proposal_created", id: 2041 }) }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledWith({ kind: "sent", proposal: 2041 }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("hands a lock to the page", async () => {
    const onSettled = vi.fn();
    renderFile(<PrDialog {...props} onSettled={onSettled} />, { api: server({ create: () => problem(423, "locked") }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledWith(expect.objectContaining({ kind: "locked" })));
  });

  it("keeps any other failure in the dialog, with the request id, and nothing created", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ create: () => problem(500, "internal") }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("req-test-1");
    expect(screen.getByRole("button", { name: "Create PR" })).toBeInTheDocument();
  });
});

describe("PrDialog: GitHub token trouble (1j, 1k)", () => {
  it("says the token expired, writes nothing, keeps the text and links My GitHub token (1j)", async () => {
    const onTokenExpired = vi.fn();
    renderFile(<PrDialog {...props} onTokenExpired={onTokenExpired} />, { api: server({ create: () => problem(422, "token_expired") }) });
    const title = await toForm();
    await userEvent.clear(title);
    await userEvent.type(title, "My change");
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    expect(await screen.findByText("Your GitHub token expired, so the PR was not created.")).toBeInTheDocument();
    expect(screen.getByText(/Renew it in My token, then retry\. Your title and description are kept\./)).toBeInTheDocument();
    expect(screen.getByText("Nothing. No branch or commit was created.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Go to My token" })).toHaveAttribute("href", "/me/github-token");
    expect(onTokenExpired).toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByLabelText("Title")).toHaveValue("My change");
  });

  it("says there is no push access, names the repository, and offers the other target (1k)", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ create: () => problem(422, "token_missing_access", { base: true, tenant: false }) }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    expect(await screen.findByText("You don't have push access to csp/csp-tenant-data, so the PR was not created.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Choose another target" }));
    expect(await screen.findByRole("radio", { name: /^Base/ })).toBeInTheDocument();
  });

  it("says a token that cannot be used cannot be used, and links the page that fixes it", async () => {
    renderFile(<PrDialog {...props} />, { api: server({ create: () => problem(422, "classic_token") }) });
    await toForm();
    await userEvent.click(screen.getByRole("button", { name: "Create PR" }));
    expect(await screen.findByText("Your GitHub token cannot be used, so the PR was not created.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Go to My token" })).toBeInTheDocument();
  });
});

describe("PrDialog: closing", () => {
  it("closes from Cancel on the first step", async () => {
    const onOpenChange = vi.fn();
    renderFile(<PrDialog {...props} onOpenChange={onOpenChange} />, { api: server() });
    await userEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});

describe("PrDialog: closed", () => {
  it("renders nothing and settles: a closed dialog does not keep re-rendering itself", async () => {
    let commits = 0;
    renderFile(
      <Profiler
        id="closed"
        onRender={() => {
          commits += 1;
          // A loop would never end inside `act`: turn it into a failure instead of a hang.
          if (commits > 50) throw new Error("a closed PR dialog keeps re-rendering");
        }}
      >
        <PrDialog {...props} open={false} />
      </Profiler>,
      { api: server() },
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 100));
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(commits).toBeLessThan(10);
  });
});
