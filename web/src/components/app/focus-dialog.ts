/**
 * `onOpenAutoFocus` for a dialog that is read first: focus starts on the dialog itself. The first control in it may be the request ID's
 * copy button, and a keyboard hint that opens on a button nobody tabbed to takes the first Escape; the footer's first action may be
 * "Discard my changes", which is not a place to start. The dialog is read first (its title and description name it), and Tab goes to
 * its content and then to the actions.
 */
export function focusDialog(event: Event): void {
  if (event.target instanceof HTMLElement) {
    event.preventDefault();
    event.target.focus();
  }
}
