import { join } from "node:path";
import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";
import { WEB_ROOT } from "../scripts/handoff/dc-parse.mjs";

const eslint = new ESLint({ cwd: WEB_ROOT, overrideConfigFile: join(WEB_ROOT, "eslint.config.js") });

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

  it("keeps the extracted handoff data out of product code, but not out of tests or tooling", async () => {
    const code = 'import { TOKENS } from "@/handoff/tokens.generated";\nexport { TOKENS };\n';
    expect(await restricted(code, "src/components/app/Foo.tsx")).toHaveLength(1);
    expect(await restricted(code, "src/features/x/Page.tsx")).toHaveLength(1);
    expect(await restricted(code, "src/components/app/Foo.test.tsx")).toEqual([]);
    expect(await restricted(code, "src/handoff/x.test.ts")).toEqual([]);
  });
});
