import { render } from "@testing-library/react";
import { Dialog } from "radix-ui";
import { describe, expect, it, vi } from "vitest";

// src/test/setup.ts waits one turn of the event loop after it unmounts a test's tree. Radix's FocusScope hands focus back from a
// `setTimeout(..., 0)` that it queues when a dialog unmounts. Left pending when the last test of a file ended, that timer fired
// after the jsdom environment was gone and Vitest reported `TypeError: Failed to execute 'dispatchEvent' on 'EventTarget':
// parameter 1 is not of type 'Event'` as an unhandled error, which failed the run with every test passing (CI, 2026-10-26,
// DeleteDialog.test.tsx). The first test closes a dialog; the second runs after the afterEach of the first and finds the
// timer already fired.
const closeAutoFocus = vi.fn();

describe("the shared afterEach", () => {
  it("unmounts a dialog, whose focus scope then queues a timer that reports the close", () => {
    render(
      <Dialog.Root open>
        <Dialog.Portal>
          <Dialog.Content onCloseAutoFocus={closeAutoFocus}>
            <Dialog.Title>Title</Dialog.Title>
            <Dialog.Description>Description</Dialog.Description>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>,
    );
    expect(closeAutoFocus).not.toHaveBeenCalled();
  });

  it("has let that timer fire before the next test starts", () => {
    expect(closeAutoFocus).toHaveBeenCalledTimes(1);
  });
});
