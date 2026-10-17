import { join } from "node:path";
import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";
import { WEB_ROOT } from "../scripts/handoff/dc-parse.mjs";
import { syntaxOnlyConfig } from "./syntax-config.mjs";

// A snippet is not a file of a tsconfig, so these run the rules that need no type information.
const eslint = new ESLint({ cwd: WEB_ROOT, overrideConfigFile: true, overrideConfig: syntaxOnlyConfig });

async function restricted(code, filePath) {
  const [result] = await eslint.lintText(code, { filePath: join(WEB_ROOT, filePath) });
  return result.messages.filter((m) => m.ruleId === "no-restricted-imports");
}

describe("imports the app must not make (S12)", () => {
  it.each([
    ['import { Select } from "radix-ui";'],
    ['import { ScrollArea } from "radix-ui";'],
    ['import { Dialog, Select } from "radix-ui";'],
    ['import * as Select from "@radix-ui/react-select";'],
    ['import * as ScrollArea from "@radix-ui/react-scroll-area";'],
    ['import { toast } from "sonner";'],
  ])("bans %s in product code", async (code) => {
    expect(await restricted(`${code}\nexport {};\n`, "src/components/ui/x.tsx")).toHaveLength(1);
  });

  it("bans them in tests too", async () => {
    expect(await restricted('import { toast } from "sonner";\nexport {};\n', "src/components/ui/x.test.tsx")).toHaveLength(1);
    expect(await restricted('import { Select } from "radix-ui";\nexport {};\n', "src/components/ui/x.test.tsx")).toHaveLength(1);
  });

  it("allows the Radix parts that read clean", async () => {
    const code = 'import { Dialog, Popover, Tooltip, DropdownMenu, Tabs, ToggleGroup, Slot } from "radix-ui";\nexport { Dialog, Popover, Tooltip, DropdownMenu, Tabs, ToggleGroup, Slot };\n';
    expect(await restricted(code, "src/components/ui/x.tsx")).toEqual([]);
  });

  it("bans the router's own lazy component, which reloads the page and uses session storage", async () => {
    const code = 'import { lazyRouteComponent } from "@tanstack/react-router";\nexport { lazyRouteComponent };\n';
    expect(await restricted(code, "src/router.tsx")).toHaveLength(1);
    expect(await restricted(code, "src/testpages/x.tsx")).toHaveLength(1);
    expect(await restricted('import { createRoute, Link } from "@tanstack/react-router";\nexport { createRoute, Link };\n', "src/router.tsx")).toEqual([]);
  });

  it("keeps the extracted handoff data out of product code, but not out of tests or tooling", async () => {
    const code = 'import { TOKENS } from "@/handoff/tokens.generated";\nexport { TOKENS };\n';
    expect(await restricted(code, "src/components/app/Foo.tsx")).toHaveLength(1);
    expect(await restricted(code, "src/features/x/Page.tsx")).toHaveLength(1);
    expect(await restricted(code, "src/components/app/Foo.test.tsx")).toEqual([]);
    expect(await restricted(code, "src/handoff/x.test.ts")).toEqual([]);
  });

  it.each([
    ["zustand", 'import { create } from "zustand";'],
    ["zustand subpath", 'import { createStore } from "zustand/vanilla";'],
    ["redux", 'import { createStore } from "redux";'],
    ["redux toolkit", 'import { configureStore } from "@reduxjs/toolkit";'],
    ["react-redux", 'import { useSelector } from "react-redux";'],
    ["jotai", 'import { atom } from "jotai";'],
    ["valtio", 'import { proxy } from "valtio";'],
    ["recoil", 'import { atom } from "recoil";'],
    ["mobx", 'import { observable } from "mobx";'],
    ["mobx-react-lite", 'import { observer } from "mobx-react-lite";'],
  ])("bans a client-state library, once, with the reason: %s", async (_name, code) => {
    for (const file of ["src/features/x/Page.tsx", "src/lib/query/x.ts", "src/components/app/X.test.tsx", "src/mocks/x.ts"]) {
      const found = await restricted(`${code}\nexport {};\n`, file);
      expect(found, file).toHaveLength(1);
      expect(found[0]?.message, file).toMatch(/QueryClient/);
    }
  });

  it("does not ban what only sounds like one (the names are exact, and TanStack Query is the store)", async () => {
    const code = 'import { useQuery, QueryClient } from "@tanstack/react-query";\nimport { reduce } from "lodash-es";\nexport { useQuery, QueryClient, reduce };\n';
    expect(await restricted(code, "src/features/x/Page.tsx")).toEqual([]);
  });
});
