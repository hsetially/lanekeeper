import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { PathList } from "./PathList";
import { ServiceList } from "./ServiceList";

// The two lists a confirmation shows: the files about to be announced, and the services a restart would take away. Both carry
// text that came from the cluster or the file system, so it is drawn with SafeText, and both can be long, so they scroll in a
// region a keyboard can reach.

describe("PathList", () => {
  it("lists every path, in the order it was given, in a region named for what it holds", () => {
    render(<PathList label="Files to announce" paths={["application.properties", "tx-infinity-api/a.yml"]} />);
    const region = screen.getByRole("region", { name: "Files to announce" });
    expect(within(region).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["application.properties", "tx-infinity-api/a.yml"]);
  });

  it("can be scrolled with a keyboard", () => {
    render(<PathList label="Files" paths={["a.yml"]} />);
    expect(screen.getByRole("region", { name: "Files" })).toHaveAttribute("tabindex", "0");
  });

  it("shows an invisible character in a path as a visible marker, never as nothing", () => {
    render(<PathList label="Files" paths={["core/a‮ml.yml"]} />);
    expect(screen.getByRole("img", { name: /U\+202E/ })).toBeInTheDocument();
  });

  it("draws nothing for no path", () => {
    const { container } = render(<PathList label="Files" paths={[]} />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe("ServiceList", () => {
  const services = [
    { service: { namespace: "sitb", name: "tx-infinity-core" }, note: "notifications reported on" },
    { service: { namespace: "sitb", name: "config-server" } },
  ];

  it("lists each service as namespace and name, with the note it was given", () => {
    render(<ServiceList label="Affected services" services={services} />);
    const region = screen.getByRole("region", { name: "Affected services" });
    const items = within(region).getAllByRole("listitem");
    expect(items.map((li) => li.textContent)).toEqual(["sitb/tx-infinity-core notifications reported on", "sitb/config-server"]);
  });

  it("shows an invisible character in a name as a visible marker", () => {
    render(<ServiceList label="Services" services={[{ service: { namespace: "sitb", name: "a​b" } }]} />);
    expect(screen.getByRole("img", { name: /U\+200B/ })).toBeInTheDocument();
  });

  it("can be scrolled with a keyboard, and draws nothing for no service", () => {
    const { container, rerender } = render(<ServiceList label="Services" services={services} />);
    expect(screen.getByRole("region", { name: "Services" })).toHaveAttribute("tabindex", "0");
    rerender(<ServiceList label="Services" services={[]} />);
    expect(container).toBeEmptyDOMElement();
  });
});
