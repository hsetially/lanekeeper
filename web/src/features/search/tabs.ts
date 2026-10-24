import type { SearchParams } from "./url";

/** Change the search screen's parameters: `replace` for typing (no history entry for each pause), `push` for a choice. */
export type Commit = (patch: Partial<SearchParams>, how: "replace" | "push") => void;
