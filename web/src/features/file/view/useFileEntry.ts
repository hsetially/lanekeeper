import { useEffect, useMemo } from "react";
import type { FileEntry } from "@/api/generated/model";
import type { ApiProblem } from "@/api/problem";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { fileTreeQuery } from "../queries";
import { splitPath } from "../model";

// The listing entry of one file: its drift state, who changed it last, its severity, its logical file and its tenant. The
// contract has no read of one entry, so the listing of the file's folder is read (the same cache entry the tree shows, so it is
// usually there already), page by page, until the file is found or the folder ends. Bounded: a folder of more than
// MAX_PAGES pages is not read to the end, and the file is then reported as missing from the listing (the header shows what
// the file's own content says and nothing else).

const MAX_PAGES = 10;

export type EntryState =
  | { status: "loading" }
  | { status: "found"; entry: FileEntry }
  | { status: "missing" }
  | { status: "error"; error: ApiProblem };

export function useFileEntry(id: string, path: string): EntryState {
  const { dir } = splitPath(path);
  const pages = useCursorPages({ ...fileTreeQuery(id, dir, null), maxPages: MAX_PAGES });
  const entry = useMemo(() => pages.items.find((item) => item.path === path && !item.is_dir), [pages.items, path]);
  const { hasMore, isFetching, loadMore, status } = pages;

  useEffect(() => {
    if (status === "success" && entry === undefined && hasMore && !isFetching) void loadMore();
  }, [status, entry, hasMore, isFetching, loadMore]);

  if (entry !== undefined) return { status: "found", entry };
  if (status === "error" && pages.error) return { status: "error", error: pages.error };
  if (status === "pending" || hasMore) return { status: "loading" };
  return { status: "missing" };
}
