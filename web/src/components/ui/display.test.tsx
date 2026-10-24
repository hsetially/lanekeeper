import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { Alert, AlertDescription, AlertTitle } from "./alert";
import { Badge } from "./badge";
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbPage, BreadcrumbSeparator } from "./breadcrumb";
import { Kbd } from "./kbd";
import { Separator } from "./separator";
import { Skeleton } from "./skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger, TabCount } from "./tabs";

describe("Tabs", () => {
  it("moves between tabs with the arrow keys and shows the matching panel", async () => {
    const user = userEvent.setup();
    render(
      <Tabs defaultValue="files">
        <TabsList aria-label="Swimlane">
          <TabsTrigger value="files">
            Files <TabCount>412</TabCount>
          </TabsTrigger>
          <TabsTrigger value="findings">
            Findings <TabCount>8</TabCount>
          </TabsTrigger>
        </TabsList>
        <TabsContent value="files">file list</TabsContent>
        <TabsContent value="findings">finding list</TabsContent>
      </Tabs>,
    );
    expect(screen.getByRole("tablist", { name: "Swimlane" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /Files/ })).toHaveAttribute("aria-selected", "true");
    screen.getByRole("tab", { name: /Files/ }).focus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: /Findings/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("finding list")).toBeVisible();
    expect(screen.queryByText("file list")).not.toBeInTheDocument();
  });
});

describe("Alert", () => {
  it("is a polite status for info, success and warning, and an alert for danger", () => {
    const { rerender } = render(
      <Alert variant="warning">
        <AlertTitle>NFS ahead</AlertTitle>
        <AlertDescription>The file differs from Git.</AlertDescription>
      </Alert>,
    );
    expect(screen.getByRole("status")).toHaveTextContent("NFS ahead");
    rerender(
      <Alert variant="danger">
        <AlertTitle>Could not save</AlertTitle>
      </Alert>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("Could not save");
  });

  it("carries an icon, so colour is never the only signal", () => {
    for (const variant of ["info", "success", "warning", "danger"] as const) {
      const { container, unmount } = render(
        <Alert variant={variant}>
          <AlertTitle>t</AlertTitle>
        </Alert>,
      );
      expect(container.querySelector("svg")).not.toBeNull();
      unmount();
    }
  });
});

describe("small pieces", () => {
  it("Breadcrumb marks the current page", () => {
    render(
      <Breadcrumb>
        <BreadcrumbList>
          <BreadcrumbItem>
            <BreadcrumbLink href="/swimlanes">ops-sit</BreadcrumbLink>
          </BreadcrumbItem>
          <BreadcrumbSeparator />
          <BreadcrumbItem>
            <BreadcrumbPage>sitb</BreadcrumbPage>
          </BreadcrumbItem>
        </BreadcrumbList>
      </Breadcrumb>,
    );
    expect(screen.getByRole("navigation", { name: "Breadcrumb" })).toBeInTheDocument();
    expect(screen.getByText("sitb")).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("link", { name: "ops-sit" })).toHaveAttribute("href", "/swimlanes");
  });

  it("Kbd, Badge, Separator and Skeleton render", () => {
    const { container } = render(
      <div>
        <Kbd>⌘K</Kbd>
        <Badge variant="outline">Viewer</Badge>
        <Separator />
        <Skeleton data-testid="sk" className="h-4 w-20" />
      </div>,
    );
    expect(screen.getByText("⌘K").tagName).toBe("KBD");
    expect(screen.getByText("Viewer")).toHaveAttribute("data-variant", "outline");
    expect(container.querySelector('[role="none"], [role="separator"]')).not.toBeNull();
    expect(screen.getByTestId("sk")).toHaveAttribute("aria-hidden", "true");
  });
});
