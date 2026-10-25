import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { SettingChange } from "@/api/generated/model";
import { ChangeList, SettingChips } from "./ChangeSummary";
import { summarise } from "./summary";

const value = (text: string) => ({ text, ty: "string" as const, flag: null });
const changes: SettingChange[] = [
  { path: "txInfinityOptions.remote-itm-teller.enableAccountSorting", kind: "changed", left: value("false"), right: value("true"), left_location: null, right_location: { path: "x", lines: { start: 13, end: 13 } }, severity: null },
  { path: "txInfinityOptions.remote-itm-teller.sortOrder", kind: "added", left: null, right: value("asc"), left_location: null, right_location: { path: "x", lines: { start: 14, end: 14 } }, severity: null },
  { path: "logging.level", kind: "changed", left: value("WARN"), right: value("INFO"), left_location: null, right_location: { path: "x", lines: { start: 31, end: 31 } }, severity: null },
];

describe("SettingChips", () => {
  it("says what changed, then one chip per setting with the old and the new value", () => {
    render(<SettingChips summary={summarise(changes)} />);
    expect(screen.getByText("2 settings changed, 1 added")).toBeInTheDocument();
    const chips = screen.getAllByRole("listitem");
    expect(chips).toHaveLength(3);
    expect(within(chips[0] as HTMLElement).getByText("enableAccountSorting")).toBeInTheDocument();
    expect(within(chips[0] as HTMLElement).getByText("false")).toBeInTheDocument();
    expect(within(chips[0] as HTMLElement).getByText("true")).toBeInTheDocument();
    expect(within(chips[1] as HTMLElement).queryByText("false")).not.toBeInTheDocument();
  });
  it("is nothing when there are no settings to speak of (a plain text file)", () => {
    const { container } = render(<SettingChips summary={summarise([])} />);
    expect(container).toBeEmptyDOMElement();
  });
  it("says how many more there are than it shows", () => {
    const many = Array.from({ length: 20 }, (_, i): SettingChange => ({ ...(changes[1] as SettingChange), path: `k${i}` }));
    render(<SettingChips summary={summarise(many)} />);
    expect(screen.getByText("+8 more")).toBeInTheDocument();
  });
  it("draws values as data, and a masked value as the marker the server sent", () => {
    const masked: SettingChange[] = [{ ...(changes[0] as SettingChange), path: "k.secret", left: value("[REDACTED: 12 chars, aws_key]"), right: value("a‮b") }];
    render(<SettingChips summary={summarise(masked)} />);
    expect(screen.getByText("[REDACTED: 12 chars, aws_key]")).toBeInTheDocument();
    expect(screen.getByText("<U+202E>")).toBeInTheDocument();
  });
});

describe("ChangeList", () => {
  it("counts the changes and gives each its kind and its line", () => {
    render(<ChangeList summary={summarise(changes)} />);
    expect(screen.getByText("3 changes")).toBeInTheDocument();
    const items = screen.getAllByRole("listitem");
    expect(items).toHaveLength(3);
    expect(items[1]).toHaveTextContent("txInfinityOptions.remote-itm-teller.sortOrder");
    expect(items[1]).toHaveTextContent("Added");
    expect(items[1]).toHaveTextContent("line 14");
  });
  it("says 1 change in the singular", () => {
    render(<ChangeList summary={summarise(changes.slice(0, 1))} />);
    expect(screen.getByText("1 change")).toBeInTheDocument();
  });
  it("leaves the line out when the server gave none", () => {
    render(<ChangeList summary={summarise([{ ...(changes[0] as SettingChange), right_location: null }])} />);
    expect(screen.getByRole("listitem")).not.toHaveTextContent("line");
  });
});
