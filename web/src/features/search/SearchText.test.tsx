import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TextHit } from "@/api/generated/model";
import { LANES, textHitsFor } from "@/mocks/design/search/data";
import { at, hub, searchBackend, type Route } from "@/mocks/design/search/hub";
import { json, problem, renderApp } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// Text search: the Text tab of the Search screen. The query lives in the address, a request is made for it and for nothing
// else, and every state of the States index (loading, empty, error, truncated) and the design's own (invalid regex, no
// results with suggestions) is here.

configure({ asyncUtilTimeout: 5000 });
// A test boots the whole app (router, session, shell) and then types and clicks; the default five seconds is too tight on a busy machine.
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const textCalls = (calls: ReadonlyArray<{ path: string }>) => calls.filter((c) => c.path.startsWith("/api/v1/search/text")).map((c) => new URL(c.path, "http://x").searchParams);
// The searches the person asked for, not the ones that count what a suggestion would find (they ask for at most 100).
const askedCalls = (calls: ReadonlyArray<{ path: string }>) => textCalls(calls).filter((p) => !p.has("max_total"));

function open(url: string, routes: Route[] = searchBackend(), role = "viewer") {
  return renderApp(url, hub(role, routes));
}

const tree = () => screen.findByRole("tree", { name: "Results by swimlane and file" });
const query = () => screen.getByRole("searchbox", { name: "Query" });

describe("a text search from the address", () => {
  it("asks for the query and shows the hits by swimlane and then by file, with the match marked", async () => {
    const app = open("/search?type=text&q=enableAccountSorting");
    const results = await tree();
    expect(textCalls(app.calls)).toHaveLength(1);
    const asked = textCalls(app.calls)[0];
    expect(asked?.get("pattern")).toBe("enableAccountSorting");
    expect(asked?.has("regex")).toBe(false);
    expect(asked?.has("case_sensitive")).toBe(false);
    expect(asked?.has("swimlanes")).toBe(false);
    expect(asked?.has("path_glob")).toBe(false);

    const lanes = within(results).getAllByRole("treeitem", { expanded: true }).filter((el) => el.getAttribute("aria-level") === "1");
    expect(lanes).toHaveLength(6);
    expect(lanes[1]).toHaveTextContent("sitb");
    expect(lanes[1]).toHaveTextContent("4 matches · 3 files");
    // The tenant file and the base file side by side, the match marked in each hit.
    const marks = [...results.querySelectorAll("mark")];
    expect(marks).toHaveLength(23);
    expect(marks.every((m) => m.textContent === "enableAccountSorting")).toBe(true);
    expect(screen.getByText("23 matches in 16 files · 6 swimlanes")).toBeInTheDocument();
    expect(screen.getByText(/Literal · ignoring case · searching 6 of 6 swimlanes/)).toBeInTheDocument();
  });

  it("links each hit to its file at its line", async () => {
    open("/search?type=text&q=enableAccountSorting");
    await tree();
    const links = screen.getAllByRole("link", { name: /^sitb tx-infinity-core-sit7\.yml line 13$/ });
    expect(links).toHaveLength(1);
    expect(links[0]).toHaveAttribute("href", "/swimlanes/sitb/files?line=13&path=tx-infinity-api%2Ftx-infinity-core-sit7.yml");
  });

  it("goes to the file at the line when a hit is chosen with the pointer", async () => {
    const app = open("/search?type=text&q=enableAccountSorting");
    await tree();
    await userEvent.click(screen.getByRole("link", { name: /^sitb tx-infinity-core-sit7\.yml line 13$/ }));
    await waitFor(() => expect(app.location()).toBe("/swimlanes/sitb/files?line=13&path=tx-infinity-api%2Ftx-infinity-core-sit7.yml"));
  });

  it("marks the match at the byte offsets the server gave, also after a letter that is more than one byte", async () => {
    const hits: TextHit[] = [{ swimlane: "sit", path: "a/b.yml", line: 3, text: "größe: needle here", ranges: [[9, 15]] }];
    open("/search?type=text&q=needle", [at("GET", "/api/v1/search/text", () => json({ hits, truncated: false }))]);
    const results = await tree();
    expect([...results.querySelectorAll("mark")].map((m) => m.textContent)).toEqual(["needle"]);
  });

  it("shows a hidden character in a hit as a visible marker and never as itself", async () => {
    const hits: TextHit[] = [{ swimlane: "sit", path: "a/b.yml", line: 1, text: "name: ev‮il", ranges: [[0, 4]] }];
    open("/search?type=text&q=name", [at("GET", "/api/v1/search/text", () => json({ hits, truncated: false }))]);
    const results = await tree();
    expect(results.textContent).toContain("<U+202E>");
    expect(results.textContent).not.toContain("‮");
  });

  it("shows a masked value as the server sent it, and says nothing about what masking does to a search", async () => {
    const hits: TextHit[] = [{ swimlane: "sit", path: "a/b.yml", line: 7, text: "password: [REDACTED: 12 chars, generic_secret]", ranges: [[0, 8]] }];
    open("/search?type=text&q=password", [at("GET", "/api/v1/search/text", () => json({ hits, truncated: false }))]);
    const results = await tree();
    expect(results.textContent).toContain("[REDACTED: 12 chars, generic_secret]");
    // Search matches the stored text; the screen must not imply that a hidden value cannot be probed this way.
    expect(document.body.textContent).not.toMatch(/(hidden|masked|redacted)[^.]{0,60}(not|never|cannot|can't)[^.]{0,40}(search|match|found)/i);
    expect(document.body.textContent).not.toMatch(/(not|never|cannot|can't)[^.]{0,40}(search|match|find)[^.]{0,60}(hidden|masked|redacted)/i);
  });

  it("draws only the rows in view of a long answer", async () => {
    const hits: TextHit[] = Array.from({ length: 500 }, (_, i) => ({ swimlane: LANES[i % 6]?.id ?? "sit", path: `d/f${Math.floor(i / 6)}.yml`, line: i + 1, text: `needle ${i}`, ranges: [[0, 6]] }));
    open("/search?type=text&q=needle", [at("GET", "/api/v1/search/text", () => json({ hits, truncated: false }))]);
    const results = await tree();
    expect(results).toHaveAttribute("aria-label", "Results by swimlane and file");
    expect(results.querySelectorAll("[role=treeitem]").length).toBeLessThan(120);
    expect(screen.getByText(/500 matches in/)).toBeInTheDocument();
  });

  it("collapses and expands the swimlanes", async () => {
    open("/search?type=text&q=enableAccountSorting");
    const results = await tree();
    await userEvent.click(screen.getByRole("button", { name: "Collapse all" }));
    expect(within(results).queryAllByRole("link")).toHaveLength(0);
    expect(within(results).getAllByRole("treeitem")).toHaveLength(6);
    await userEvent.click(screen.getByRole("button", { name: "Expand all" }));
    expect(within(results).getAllByRole("link")).toHaveLength(23);
  });
});

describe("the query, the switches and the filters", () => {
  it("asks nothing until there is a query, and says what to do", async () => {
    const app = open("/search?type=text");
    expect(await screen.findByText("Search 6 swimlanes")).toBeInTheDocument();
    expect(screen.getByText(/Type a query to find it in every config file/)).toBeInTheDocument();
    expect(screen.getByText("Type to search")).toBeInTheDocument();
    expect(textCalls(app.calls)).toEqual([]);
  });

  it("puts what is typed in the address, then asks for it", async () => {
    const app = open("/search?type=text");
    await screen.findByText("Search 6 swimlanes");
    await userEvent.type(query(), "timeoutMs");
    await waitFor(() => expect(app.location()).toBe("/search?q=timeoutMs&type=text"));
    await waitFor(() => expect(textCalls(app.calls).map((p) => p.get("pattern"))).toEqual(["timeoutMs"]));
    await tree();
  });

  it("asks once for a word typed in one go, not once for each letter", async () => {
    const app = open("/search?type=text");
    await screen.findByText("Search 6 swimlanes");
    await userEvent.type(query(), "timeout");
    await tree();
    expect(textCalls(app.calls)).toHaveLength(1);
  });

  it("asks at once when Enter is pressed", async () => {
    const app = open("/search?type=text");
    await screen.findByText("Search 6 swimlanes");
    await userEvent.type(query(), "timeoutMs{Enter}");
    expect(app.location()).toBe("/search?q=timeoutMs&type=text");
  });

  it("follows the address when it changes by itself, the way Back does", async () => {
    const app = open("/search?type=text&q=timeoutMs");
    await tree();
    app.router.history.push("/search?type=text&q=level");
    await waitFor(() => expect(query()).toHaveValue("level"));
  });

  it("turns on regex and case from the toggles, in the address and in the request", async () => {
    const app = open("/search?type=text&q=enable.*Sorting");
    // Literal, the dot and the star match themselves, so nothing is found.
    expect(await screen.findByText("No matches for “enable.*Sorting”")).toBeInTheDocument();
    const caseButton = screen.getByRole("button", { name: "Match case" });
    const regexButton = screen.getByRole("button", { name: "Use regular expression" });
    expect(caseButton).toHaveAttribute("aria-pressed", "false");
    expect(regexButton).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(regexButton);
    await waitFor(() => expect(app.location()).toBe("/search?q=enable.*Sorting&rx=1&type=text"));
    await tree();
    expect(textCalls(app.calls).at(-1)?.get("regex")).toBe("true");
    await userEvent.click(caseButton);
    await waitFor(() => expect(app.location()).toContain("cs=1"));
    await waitFor(() => expect(textCalls(app.calls).at(-1)?.get("case_sensitive")).toBe("true"));
    expect(textCalls(app.calls).at(-1)?.get("regex")).toBe("true");
    expect(screen.getByRole("button", { name: "Match case" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Use regular expression" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(/Regex · case sensitive/)).toBeInTheDocument();
  });

  it("sends the path glob", async () => {
    const app = open("/search?type=text&q=enableAccountSorting&glob=**/*-sit7.yml");
    const results = await tree();
    expect(textCalls(app.calls)[0]?.get("path_glob")).toBe("**/*-sit7.yml");
    expect([...results.querySelectorAll("[role=treeitem]")].filter((r) => r.getAttribute("aria-level") === "2")).toHaveLength(1);
    expect(screen.getByLabelText("Path glob")).toHaveValue("**/*-sit7.yml");
    expect(screen.getByText(/· \*\*\/\*-sit7\.yml/)).toBeInTheDocument();
  });

  it("names a pattern that is too long for the server and does not send it", async () => {
    const app = open(`/search?type=text&q=${"é".repeat(300)}`);
    // The summary line and the empty state both say so.
    expect(await screen.findAllByText("Pattern not searched")).toHaveLength(2);
    expect(screen.getByRole("alert")).toHaveTextContent("longer than 512 bytes");
    expect(textCalls(app.calls)).toEqual([]);
  });
});

describe("the swimlanes", () => {
  it("lets the picker's footer wrap, so that 'Counts are matches per swimlane' is not cut off by the box when the words are longer (the layout run)", async () => {
    open("/search?type=text&q=enableAccountSorting");
    await tree();
    await userEvent.click(await screen.findByRole("button", { name: /Swimlanes All 6/ }));
    const footer = (await screen.findByText("Counts are matches per swimlane")).parentElement;
    expect(footer?.className).toMatch(/(^|\s)flex-wrap(\s|$)/);
    expect(footer?.className).toMatch(/(^|\s)min-h-8(\s|$)/);
    expect(footer?.className).not.toMatch(/(^|\s)h-8(\s|$)/);
  });

  it("lists the swimlanes with the matches in each, and searches the ones that stay checked", async () => {
    const app = open("/search?type=text&q=enableAccountSorting");
    await tree();
    await userEvent.click(await screen.findByRole("button", { name: /Swimlanes All 6/ }));
    const group = await screen.findByRole("group", { name: "Swimlanes" });
    expect(screen.getByText("6 of 6 selected")).toBeInTheDocument();
    expect(within(group).getByRole("checkbox", { name: /\bsitb\b/ })).toHaveAccessibleDescription("4 matches");
    await userEvent.click(within(group).getByRole("checkbox", { name: /presitc/ }));
    await waitFor(() => expect(app.location()).toContain("lanes=sit%2Csitb%2Csitc%2Cpresita%2Cpresitb"));
    await waitFor(() => expect(textCalls(app.calls).at(-1)?.get("swimlanes")).toBe("sit,sitb,sitc,presita,presitb"));
    await waitFor(() => expect(screen.getByText(/searching 5 of 6 swimlanes/)).toBeInTheDocument());
  });

  it("selects none, and says so instead of asking", async () => {
    const app = open("/search?type=text&q=enableAccountSorting");
    await tree();
    await userEvent.click(await screen.findByRole("button", { name: /Swimlanes All 6/ }));
    await userEvent.click(await screen.findByRole("button", { name: "Clear" }));
    expect((await screen.findAllByText("No swimlanes selected")).length).toBeGreaterThan(0);
    expect(app.location()).toContain("lanes=-");
    expect(textCalls(app.calls)).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: "Select all" }));
    await tree();
    expect(textCalls(app.calls)).toHaveLength(2);
  });

  it("reads the swimlanes from the address, and ignores what is not a swimlane", async () => {
    const app = open("/search?type=text&q=enableAccountSorting&lanes=sitb,../x");
    await tree();
    expect(textCalls(app.calls)[0]?.get("swimlanes")).toBe("sitb");
    expect(screen.getByRole("button", { name: /Swimlanes sitb/ })).toBeInTheDocument();
  });
});

describe("every state", () => {
  it("shows a skeleton while the search runs, named and busy", async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const api = hub("viewer", searchBackend());
    renderApp("/search?type=text&q=timeoutMs", async (call) => {
      if (call.path.startsWith("/api/v1/search/text")) await held;
      return api(call);
    });
    expect(await screen.findByLabelText("Loading search results")).toHaveAttribute("aria-busy", "true");
    release();
    await tree();
  });

  it("says what failed, with the request id, and tries again", async () => {
    let fail = true;
    const routes: Route[] = [at("GET", "/api/v1/search/text", () => (fail ? problem(500, "internal") : json({ hits: textHitsFor("timeoutMs"), truncated: false })))];
    open("/search?type=text&q=timeoutMs", routes);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't load search results");
    expect(alert).toHaveTextContent("req-test-1");
    fail = false;
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await tree();
  });

  it("says a pattern the server refused was not searched, and offers plain text", async () => {
    const app = open("/search?type=text&q=enableAccount(Sort&rx=1");
    expect(await screen.findAllByText("Pattern not searched")).toHaveLength(2);
    const alert = screen.getAllByRole("alert").find((a) => a.textContent?.includes("Invalid regular expression")) as HTMLElement;
    expect(alert).toHaveTextContent("Invalid regular expression");
    expect(screen.getByLabelText("Query")).toHaveAttribute("aria-invalid", "true");
    await userEvent.click(within(alert).getByRole("button", { name: "Search as plain text" }));
    await waitFor(() => expect(askedCalls(app.calls).at(-1)?.has("regex")).toBe(false));
    expect(app.location()).not.toContain("rx=1");
  });

  it("says the results stop at the cap the server applied, and how many are listed", async () => {
    const hits = Array.from({ length: 500 }, (_, i) => ({ swimlane: LANES[i % 6]?.id ?? "sit", path: `d/f${i % 40}.yml`, line: i + 1, text: `x ${i}`, ranges: [[0, 1]] }));
    open("/search?type=text&q=x", [at("GET", "/api/v1/search/text", () => json({ hits, truncated: true }))]);
    await tree();
    const banner = screen.getByText("Results cut off").closest("[role=status]");
    expect(banner).toHaveTextContent("Showing the first 500 matches.");
    expect(screen.getByText("500+ matches")).toBeInTheDocument();
    expect(screen.getByText(/Results stop here/)).toBeInTheDocument();
  });

  it("says some lines of a file were left out when the server's cap on one file cut the answer", async () => {
    const hits = textHitsFor("enableAccountSorting").slice(0, 3);
    open("/search?type=text&q=enableAccountSorting", [at("GET", "/api/v1/search/text", () => json({ hits, truncated: true }))]);
    await tree();
    expect(screen.getByText("Some files have more matching lines than are listed.")).toBeInTheDocument();
    expect(screen.queryByText(/Results stop here/)).toBeNull();
    expect(screen.getByText("3+ matches")).toBeInTheDocument();
  });

  it("says what was searched when nothing matched", async () => {
    open("/search?type=text&q=ENABLEACCOUNTSORTING&cs=1&lanes=presita&glob=**/*-sit7.yml");
    expect(await screen.findByText("No matches for “ENABLEACCOUNTSORTING”")).toBeInTheDocument();
    expect(screen.getByText(/Searched 1 of 6 swimlanes\. Literal text, case sensitive, paths matching \*\*\/\*-sit7\.yml\./)).toBeInTheDocument();
  });

  it.each([
    ["literal text that is a pattern", "q=enable.*Sorting&lanes=presita,presitc", "Switch to regex", { has: "rx=1" }],
    ["the wrong case", "q=ENABLEACCOUNTSORTING&cs=1", "Ignore case", { gone: "cs=1" }],
    ["a glob that matches no file", "q=enableAccountSorting&glob=**/nothing.yml", "Clear the path glob", { gone: "glob=" }],
    ["swimlanes that do not have it", "q=60000&lanes=presita,presitb", "Search all 6 swimlanes", { gone: "lanes=" }],
  ] as const)("offers a search that would match, with how many it finds: %s", async (_name, query, label, change) => {
    const app = open(`/search?type=text&${query}`);
    const box = await screen.findByRole("region", { name: "Try instead" });
    const item = within(box).getByText(label).closest("li") as HTMLElement;
    await waitFor(() => expect(item).toHaveTextContent(/\d+ matches? in \d+ files?/));
    // It is a real search: the original, then one that changes one thing, asking for few results.
    const counted = textCalls(app.calls).slice(1);
    expect(counted.length).toBeGreaterThanOrEqual(1);
    expect(counted.every((p) => p.get("max_total") === "100")).toBe(true);
    await userEvent.click(within(item).getByRole("button", { name: "Try this" }));
    await tree();
    if ("has" in change) expect(app.location()).toContain(change.has);
    else expect(app.location()).not.toContain(change.gone);
  });

  it("explains how literal search works when nothing would help", async () => {
    open("/search?type=text&q=zzzzzz");
    expect(await screen.findByText("No matches for “zzzzzz”")).toBeInTheDocument();
    expect(screen.getByText(/Search is literal by default, so “\.” and “\*” match themselves/)).toBeInTheDocument();
    expect(screen.queryByText("Try instead")).toBeNull();
  });
});

describe("the keyboard", () => {
  it("focuses the query on /, and not while a field has the focus", async () => {
    open("/search?type=text&q=timeoutMs");
    await tree();
    expect(query()).not.toHaveFocus();
    await userEvent.keyboard("/");
    await waitFor(() => expect(query()).toHaveFocus());
    expect(query()).toHaveValue("timeoutMs");
    await userEvent.keyboard("{Escape}");
    await userEvent.type(query(), "/");
    expect(query()).toHaveValue("timeoutMs/");
  });
});

describe("structure for assistive technology", () => {
  it("has no structural violation with results, with no match, and with a pattern the server refused", async () => {
    const app = open("/search?type=text&q=enableAccountSorting");
    await tree();
    await expectNoAxeViolations();
    app.router.history.push("/search?type=text&q=zzzzzz");
    await screen.findByText("No matches for “zzzzzz”");
    await expectNoAxeViolations();
    app.router.history.push("/search?type=text&q=enableAccount(Sort&rx=1");
    await screen.findAllByText("Pattern not searched");
    await expectNoAxeViolations();
  });

  it("has no structural violation with the swimlane list open", async () => {
    open("/search?type=text&q=enableAccountSorting");
    await tree();
    await userEvent.click(await screen.findByRole("button", { name: /Swimlanes All 6/ }));
    await screen.findByRole("group", { name: "Swimlanes" });
    await expectNoAxeViolations();
  });
});
