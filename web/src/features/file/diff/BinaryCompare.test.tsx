import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { BinaryCompare } from "./BinaryCompare";

// Two versions of a binary file, side by side (Diff, 1m): "Different content", each side's picture from the blob store by its
// hash, with the hash and the size. Binary files compare by image, not by line.

const LEFT = "3af02d8" + "0".repeat(57);
const RIGHT = "b71c4e9" + "0".repeat(57);
const props = { name: "receipt-logo.jpeg", leftLabel: "Git (sit7 head)", rightLabel: "NFS (current)", binary: { left: LEFT, right: RIGHT, left_size: 16 * 1024, right_size: 18 * 1024 } };

describe("BinaryCompare", () => {
  it("says the content differs and that the comparison is by image", () => {
    render(<BinaryCompare {...props} />);
    expect(screen.getByText("Different content")).toBeInTheDocument();
    expect(screen.getByText("Same name, different hash. Binary files compare by image, not by line.")).toBeInTheDocument();
  });

  it("draws each side from the blob store by its hash", () => {
    render(<BinaryCompare {...props} />);
    expect(screen.getByRole("img", { name: "receipt-logo.jpeg from Git (sit7 head)" })).toHaveAttribute("src", `/api/v1/blobs/${LEFT}`);
    expect(screen.getByRole("img", { name: "receipt-logo.jpeg from NFS (current)" })).toHaveAttribute("src", `/api/v1/blobs/${RIGHT}`);
  });

  it("shows both hashes and both sizes", () => {
    render(<BinaryCompare {...props} />);
    expect(screen.getByText("3af02d8")).toBeInTheDocument();
    expect(screen.getByText("b71c4e9")).toBeInTheDocument();
    expect(screen.getByText("16 KB")).toBeInTheDocument();
    expect(screen.getByText("18 KB")).toBeInTheDocument();
  });

  it("says when a side has no such file", () => {
    render(<BinaryCompare {...props} binary={{ left: null, right: RIGHT, left_size: null, right_size: 100 }} />);
    expect(screen.getByText("Not in Git (sit7 head)")).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: /from Git/ })).not.toBeInTheDocument();
  });

  it("does not draw what is not an image, and does not trust a hash that is not one", () => {
    render(<BinaryCompare {...props} name="keystore.p12" />);
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    render(<BinaryCompare {...props} binary={{ left: "../x", right: "y".repeat(64), left_size: 1, right_size: 1 }} />);
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
  });
});
