import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { KIND_EXAMPLES } from "@/mocks/design/findings/fixtures";
import { renderApp } from "@/test/app";
import { KINDS, KIND_INFO, codeOf } from "./kinds";
import { SEVERITY_LABEL } from "./severity";
import { cellsOf, tableRows } from "@/mocks/design/findings/dom";
import { isFindings, newWorld, queryOf, worldApi, type World } from "@/mocks/design/findings/world";

// Every kind of finding of the contract, C1 to C12, on the screen: its row, its words in the drawer (what it means, what to do),
// and the places the drawer can take the person. One finding of each kind is served; the test opens them one after the other.

configure({ asyncUtilTimeout: 8_000 });
vi.setConfig({ testTimeout: 60_000 });

let world: World;
beforeEach(() => {
  world = newWorld(Object.values(KIND_EXAMPLES), "viewer");
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const rowOfKind = (code: string, name: string): HTMLElement => {
  const row = tableRows().find((r) => cellsOf(r)[1] === `${code}${name}`);
  if (row === undefined) throw new Error(`no row for ${code}`);
  return row;
};

describe("each check on the screen", () => {
  it("has a row in the table with its code and its name, in the severity the server gave it", async () => {
    renderApp("/findings", worldApi(world));
    await waitFor(() => expect(tableRows()).toHaveLength(12));
    for (const kind of KINDS) {
      const example = KIND_EXAMPLES[kind];
      const row = rowOfKind(codeOf(kind), KIND_INFO[kind].name);
      expect(cellsOf(row)[0], kind).toBe(SEVERITY_LABEL[example.severity]);
      expect(cellsOf(row)[4], kind).toBe(example.message);
    }
  });

  it("opens in the drawer with its name, its meaning and its steps, and the actions that fit it", async () => {
    renderApp("/findings", worldApi(world));
    await waitFor(() => expect(tableRows()).toHaveLength(12));
    for (const kind of KINDS) {
      const info = KIND_INFO[kind];
      const example = KIND_EXAMPLES[kind];
      await userEvent.click(rowOfKind(info.code, info.name));
      const drawer = await screen.findByRole("dialog", { name: info.name });
      expect(within(drawer).getByText(info.code), kind).toBeInTheDocument();
      expect(within(drawer).getByText(`${SEVERITY_LABEL[example.severity]} severity`), kind).toBeInTheDocument();
      expect(within(drawer).getByText(info.meaning), kind).toBeInTheDocument();
      for (const step of info.steps) expect(within(drawer).getByText(step), `${kind}: ${step}`).toBeInTheDocument();
      expect(within(drawer).getByText(example.message), kind).toBeInTheDocument();

      // Where it can take the person: the file, if the finding is about one; the diff only where Git is the comparison; the
      // history only where a version was lost.
      const files = within(drawer).queryAllByRole("link", { name: /^Open (file|diff|history)$/ }).map((link) => link.textContent);
      const expected = example.path === null ? [] : ["Open file", ...(info.opens.includes("diff") ? ["Open diff"] : []), ...(info.opens.includes("history") ? ["Open history"] : [])];
      expect(files, kind).toEqual(expected);
      if (example.path === null) expect(within(drawer).getByText("This finding is about the swimlane, not about one file."), kind).toBeInTheDocument();
    }
  });

  it("is offered by the Check filter under its code and its name, and asks the server for exactly that kind", async () => {
    const app = renderApp("/findings", worldApi(world));
    await waitFor(() => expect(tableRows()).toHaveLength(12));
    await userEvent.click(within(screen.getByRole("group", { name: "Filters" })).getByRole("button", { name: /^Check/ }));
    const items = await screen.findAllByRole("menuitemcheckbox");
    expect(items.map((item) => item.textContent)).toEqual(KINDS.map((kind) => `${codeOf(kind)} ${KIND_INFO[kind].name}`));
    for (const kind of ["config_server_restart_required", "served_mismatch"] as const) {
      await userEvent.click(screen.getByRole("menuitemcheckbox", { name: `${codeOf(kind)} ${KIND_INFO[kind].name}` }));
    }
    await waitFor(() => expect(tableRows()).toHaveLength(2));
    const last = app.calls.filter((c) => isFindings(c.path)).at(-1);
    expect(queryOf(last?.path ?? "").get("kinds")).toBe("config_server_restart_required,served_mismatch");
    expect(app.location()).toBe("/findings?code=C11%2CC12");
  });
});
