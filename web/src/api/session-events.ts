// What the fetch layer tells the rest of the app about the session. The client cannot import the router (the router
// imports the screens, which import the client), so it emits a signal and src/auth/ decides where the user goes.

export type SessionSignal =
  | { kind: "unauthenticated" }
  | { kind: "access_pending" }
  | { kind: "access_disabled" };

type Listener = (signal: SessionSignal) => void;

const MAX_LISTENERS = 8;
const listeners = new Set<Listener>();

/** Subscribe. Returns the unsubscribe function. At most 8 listeners (rule 5); the app has one. */
export function onSessionSignal(listener: Listener): () => void {
  if (listeners.size >= MAX_LISTENERS) throw new Error("too many session listeners");
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function emitSessionSignal(signal: SessionSignal): void {
  for (const listener of [...listeners]) {
    try {
      listener(signal);
    } catch {
      // One listener failing must not hide the signal from the others, or turn a 401 into an unhandled rejection.
    }
  }
}
