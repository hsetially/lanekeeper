import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { Checkbox } from "./checkbox";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "./field";
import { Input, SearchInput } from "./input";
import { NativeSelect } from "./native-select";
import { RadioGroup, RadioGroupItem } from "./radio-group";
import { Switch } from "./switch";
import { Textarea } from "./textarea";
import { ToggleGroup, ToggleGroupItem } from "./toggle-group";

describe("Field wiring", () => {
  it("associates the label, the description and the error with the control", () => {
    render(
      <Field invalid>
        <FieldLabel>Branch</FieldLabel>
        <FieldControl>
          <Input defaultValue="sit 7" />
        </FieldControl>
        <FieldDescription>Branch of csp-tenant-data.</FieldDescription>
        <FieldError>Branch names have no spaces</FieldError>
      </Field>,
    );
    const input = screen.getByLabelText("Branch");
    expect(input).toHaveAttribute("aria-invalid", "true");
    const described = (input.getAttribute("aria-describedby") ?? "").split(" ").map((id) => document.getElementById(id)?.textContent);
    expect(described).toContain("Branch of csp-tenant-data.");
    expect(described).toContain("Branch names have no spaces");
  });

  it("announces the error politely and shows it only when invalid", () => {
    const { rerender } = render(
      <Field>
        <FieldLabel>Name</FieldLabel>
        <FieldControl>
          <Input />
        </FieldControl>
        <FieldError>Required</FieldError>
      </Field>,
    );
    expect(screen.queryByText("Required")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Name")).not.toHaveAttribute("aria-invalid", "true");
    rerender(
      <Field invalid>
        <FieldLabel>Name</FieldLabel>
        <FieldControl>
          <Input />
        </FieldControl>
        <FieldError>Required</FieldError>
      </Field>,
    );
    expect(screen.getByText("Required")).toHaveAttribute("role", "alert");
  });
});

describe("Input and SearchInput", () => {
  it("is a 28px text input that takes typing", async () => {
    const user = userEvent.setup();
    render(<Input aria-label="Branch" />);
    await user.type(screen.getByLabelText("Branch"), "sitb");
    expect(screen.getByLabelText("Branch")).toHaveValue("sitb");
  });

  it("SearchInput has Match case and Regex toggles with aria-pressed", async () => {
    const user = userEvent.setup();
    const onCase = vi.fn();
    render(
      <SearchInput
        aria-label="Search text"
        kbd="/"
        toggles={[
          { id: "case", label: "Match case", short: "Aa", pressed: false, onPressedChange: onCase },
          { id: "regex", label: "Regex", short: ".*", pressed: true, onPressedChange: () => {} },
        ]}
      />,
    );
    const matchCase = screen.getByRole("button", { name: "Match case" });
    expect(matchCase).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByRole("button", { name: "Regex" })).toHaveAttribute("aria-pressed", "true");
    await user.click(matchCase);
    expect(onCase).toHaveBeenCalledWith(true);
    expect(screen.getByText("/")).toBeInTheDocument();
  });
});

describe("NativeSelect and Textarea", () => {
  it("is a real select, styled", async () => {
    const user = userEvent.setup();
    render(
      <NativeSelect aria-label="Tier" defaultValue="sit">
        <option value="sit">SIT</option>
        <option value="presit">PRESIT</option>
      </NativeSelect>,
    );
    await user.selectOptions(screen.getByLabelText("Tier"), "presit");
    expect(screen.getByLabelText("Tier")).toHaveValue("presit");
  });

  it("textarea takes text", async () => {
    const user = userEvent.setup();
    render(<Textarea aria-label="Reason" />);
    await user.type(screen.getByLabelText("Reason"), "Stale base");
    expect(screen.getByLabelText("Reason")).toHaveValue("Stale base");
  });
});

describe("Checkbox, Switch, RadioGroup, ToggleGroup", () => {
  it("Checkbox supports indeterminate (select all) as aria-checked=mixed", async () => {
    const user = userEvent.setup();
    function Boxed() {
      const [checked, setChecked] = useState<boolean | "indeterminate">("indeterminate");
      return <Checkbox aria-label="Select all" checked={checked} onCheckedChange={setChecked} />;
    }
    render(<Boxed />);
    const box = screen.getByRole("checkbox", { name: "Select all" });
    expect(box).toHaveAttribute("aria-checked", "mixed");
    await user.click(box);
    expect(box).toHaveAttribute("aria-checked", "true");
    await user.keyboard(" ");
    expect(box).toHaveAttribute("aria-checked", "false");
  });

  it("Switch toggles with Space", async () => {
    const user = userEvent.setup();
    render(<Switch aria-label="Auto notify" />);
    const sw = screen.getByRole("switch", { name: "Auto notify" });
    expect(sw).toHaveAttribute("aria-checked", "false");
    sw.focus();
    await user.keyboard(" ");
    expect(sw).toHaveAttribute("aria-checked", "true");
  });

  it("RadioGroup moves with the arrow keys", async () => {
    const user = userEvent.setup();
    render(
      <RadioGroup defaultValue="nfs" aria-label="Resolution">
        <RadioGroupItem value="nfs" aria-label="Keep NFS version" />
        <RadioGroupItem value="git" aria-label="Take Git version" />
      </RadioGroup>,
    );
    screen.getByRole("radio", { name: "Keep NFS version" }).focus();
    // Radix checks a radio when focus arrives while an arrow key is still held, so hold it across the focus move.
    await user.keyboard("{ArrowDown>}");
    await waitFor(() => expect(screen.getByRole("radio", { name: "Take Git version" })).toHaveFocus());
    await user.keyboard("{/ArrowDown}");
    expect(screen.getByRole("radio", { name: "Take Git version" })).toBeChecked();
  });

  it("ToggleGroup (segmented control) keeps one item selected", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <ToggleGroup type="single" defaultValue="split" onValueChange={onChange} aria-label="Diff view">
        <ToggleGroupItem value="split">Split</ToggleGroupItem>
        <ToggleGroupItem value="unified">Unified</ToggleGroupItem>
      </ToggleGroup>,
    );
    await user.click(screen.getByRole("radio", { name: "Unified" }));
    expect(onChange).toHaveBeenLastCalledWith("unified");
    expect(screen.getByRole("radio", { name: "Unified" })).toBeChecked();
  });
});
