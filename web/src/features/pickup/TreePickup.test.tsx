import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { PickupState } from "@/api/generated/model";
import { FilePickup } from "./FilePickup";
import { pickupRollups, type PickupNodeLike } from "./rollup";
import { TreePickup } from "./TreePickup";

// The pickup mark on a row of the file tree and on the head of a file. A file shows the state the hub gave for it, only when
// something is waiting (the tree marks drift the same way); a folder shows what its loaded files say, never "live"; the head of a
// file shows any state the hub gave, Live included. A state the hub did not give draws nothing at all.

const file = (state: PickupState | null, over: Partial<PickupNodeLike> = {}): PickupNodeLike => ({ id: "api/a.yml", isDir: false, lazy: false, more: null, entry: { pickup_state: state }, children: [], ...over });
const dir = (children: PickupNodeLike[], over: Partial<PickupNodeLike> = {}): PickupNodeLike => ({ id: "api", isDir: true, lazy: false, more: null, entry: { pickup_state: null }, children, ...over });
const drawFolder = (node: PickupNodeLike) => render(<TreePickup node={node} rollup={pickupRollups([node]).get(node.id)} />);

describe("a file in the tree", () => {
  it.each<[PickupState, string]>([
    ["needs_notify_or_restart", "Needs notify or restart"],
    ["needs_config_server_restart", "Needs config-server restart"],
    ["live_within_ttl", "Live within client cache"],
  ])("is marked when %s", (state, name) => {
    render(<TreePickup node={file(state)} rollup={undefined} />);
    const mark = screen.getByRole("img", { name });
    expect(mark.querySelector("svg")).not.toBeNull();
  });

  it("is not marked when it is live: a tree full of 'Live' marks says nothing", () => {
    const { container } = render(<TreePickup node={file("live")} rollup={undefined} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("is not marked when the hub did not state a pickup state: null is not 'Live', and it is not 'waiting' either", () => {
    const { container } = render(<TreePickup node={file(null)} rollup={undefined} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("takes the state of the row's group, when the tree merges a base file and its tenant files into one row", () => {
    render(<TreePickup node={file(null, { pickup: "needs_notify_or_restart" })} rollup={undefined} />);
    expect(screen.getByRole("img", { name: "Needs notify or restart" })).toBeInTheDocument();
  });

  it("says in the tooltip what the file's state means", async () => {
    const user = userEvent.setup();
    render(<TreePickup node={file("needs_config_server_restart")} rollup={undefined} />);
    await user.hover(screen.getByRole("img", { name: "Needs config-server restart" }));
    expect(await screen.findByRole("tooltip")).toHaveTextContent("In a new folder or channels.yml: the config-server does not serve it until it restarts.");
  });

  it("draws nothing for a 'Load more' row", () => {
    const { container } = render(<TreePickup node={{ id: "\u0000more:api", isDir: false, lazy: false, more: "api", entry: null, children: [] }} rollup={undefined} />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe("a folder in the tree", () => {
  it("is marked with the most urgent state of its loaded files and how many are in it", async () => {
    const user = userEvent.setup();
    drawFolder(dir([file("needs_notify_or_restart", { id: "api/a.yml" }), file("needs_notify_or_restart", { id: "api/b.yml" }), file("live", { id: "api/c.yml" })]));
    const mark = screen.getByRole("img", { name: "2 Needs notify or restart" });
    await user.hover(mark);
    expect(await screen.findByRole("tooltip")).toHaveTextContent("2 of the 3 files loaded in this folder need a notify or a restart.");
  });

  it("says what it did not count", async () => {
    const user = userEvent.setup();
    drawFolder(dir([file("needs_config_server_restart"), { id: "api/closed", isDir: true, lazy: true, more: null, entry: { pickup_state: null }, children: [] }]));
    await user.hover(screen.getByRole("img", { name: "1 Needs config-server restart" }));
    expect(await screen.findByRole("tooltip")).toHaveTextContent("Files that are not loaded yet are not counted.");
  });

  it("is never called live, even when every loaded file is", () => {
    const { container } = drawFolder(dir([file("live", { id: "api/a.yml" }), file("live", { id: "api/b.yml" })]));
    expect(container).toBeEmptyDOMElement();
  });

  it("says nothing when the hub stated nothing for the files", () => {
    const { container } = drawFolder(dir([file(null, { id: "api/a.yml" })]));
    expect(container).toBeEmptyDOMElement();
  });

  it("says nothing when it has no rollup (a folder the tree has not reported)", () => {
    const { container } = render(<TreePickup node={dir([])} rollup={undefined} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the state of the folder itself, with no count, when that is what the hub states", () => {
    drawFolder(dir([], { entry: { pickup_state: "needs_config_server_restart" } }));
    expect(screen.getByRole("img", { name: "Needs config-server restart" })).toBeInTheDocument();
  });
});

describe("the head of a file", () => {
  it.each<[PickupState, string]>([
    ["live", "Live"],
    ["live_within_ttl", "Live within client cache"],
    ["needs_notify_or_restart", "Needs notify or restart"],
    ["needs_config_server_restart", "Needs config-server restart"],
  ])("shows %s in full, Live included", (state, text) => {
    render(<FilePickup entry={{ pickup_state: state }} />);
    expect(screen.getByText(text)).toBeInTheDocument();
  });

  it("shows nothing when the hub did not state one, or when there is no entry yet", () => {
    const { container, rerender } = render(<FilePickup entry={{ pickup_state: null }} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<FilePickup entry={null} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<FilePickup entry={undefined} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("can be an icon alone, named for a screen reader, for a head with no room for the words", () => {
    render(<FilePickup entry={{ pickup_state: "needs_config_server_restart" }} density="compact" />);
    expect(screen.getByRole("img", { name: "Needs config-server restart" })).toBeInTheDocument();
  });

  it("says what it means for the services that read the file", async () => {
    const user = userEvent.setup();
    const { container } = render(<FilePickup entry={{ pickup_state: "needs_notify_or_restart" }} />);
    await user.hover(container.querySelector("[data-slot='pickup-chip']") as HTMLElement);
    expect(await screen.findByRole("tooltip")).toHaveTextContent("A property file: services take the new version after they are notified or restarted.");
  });
});
