import { useRef, type RefObject } from "react";

// Where focus goes when a dialog closes. Radix gives it back to the dialog's own Trigger and to nothing else; but nearly every
// dialog of the app is opened by a plain button, a row or a menu choice with `open` held in state, so there is no Trigger and focus
// would fall to the page, and a person on a keyboard would start again from the top. A dialog remembers the control that had focus
// when it opened and gives focus back to it when it closes, or to the element the caller names, whichever is still in the page.
// A caller's own `onCloseAutoFocus` that calls `preventDefault` takes the whole job over.

export type ReturnFocusProps = {
  /** Where focus goes on close instead of the control that opened the dialog (when that control is gone, or is not the right place). */
  returnFocusRef?: RefObject<HTMLElement | null> | undefined;
  onOpenAutoFocus?: ((event: Event) => void) | undefined;
  onCloseAutoFocus?: ((event: Event) => void) | undefined;
};

export function useReturnFocus({ returnFocusRef, onOpenAutoFocus, onCloseAutoFocus }: ReturnFocusProps): { onOpenAutoFocus: (event: Event) => void; onCloseAutoFocus: (event: Event) => void } {
  const opener = useRef<HTMLElement | null>(null);
  return {
    // Radix fires this as the dialog mounts, just before it moves focus inside, so the active element is still the opener.
    onOpenAutoFocus: (event) => {
      // Before the caller's own handler, which may move focus into the dialog itself (to its first field).
      const active = document.activeElement;
      opener.current = active instanceof HTMLElement && active !== document.body ? active : null;
      onOpenAutoFocus?.(event);
    },
    onCloseAutoFocus: (event) => {
      onCloseAutoFocus?.(event);
      const remembered = opener.current;
      opener.current = null;
      if (event.defaultPrevented) return;
      const target = returnFocusRef?.current ?? remembered;
      if (target?.isConnected === true) {
        // Radix would focus its Trigger here; there is none, or it is the same control.
        event.preventDefault();
        target.focus();
      }
    },
  };
}
