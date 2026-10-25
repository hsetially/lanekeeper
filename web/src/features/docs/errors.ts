import type { ApiProblem } from "@/api/problem";
import type { WriteErrorInfo } from "@/components/app/WriteError";

// What a failed write on a doc says (S21: the problem's own title and detail are never shown, only words written here, and the
// request id). Every message ends with what happened to the doc, because a failed write changed nothing.

type Action = "save" | "delete";

export function docWriteError(problem: ApiProblem, action: Action, context: { replacing?: boolean } = {}): WriteErrorInfo {
  const nothing = action === "save" ? "Nothing was saved." : "Nothing was deleted.";
  const make = (title: string, detail: string): WriteErrorInfo => ({ title, detail, ...(problem.requestId ? { requestId: problem.requestId } : {}) });
  const { status } = problem;
  if (status === 0) return make("The server could not be reached", `${nothing} Check the connection and try again.`);
  if (status === 401) return make("Your session ended", `${nothing} Sign in again and repeat it.`);
  if (status === 403) return make("You do not have the Editor role", `${nothing} Ask an Admin to grant it.`);
  if (status === 404) return make("This doc no longer exists", `Someone deleted it. ${nothing}`);
  if (status === 409) {
    return context.replacing === true
      ? make("Someone else changed it first", `${nothing} The doc was reloaded. Read the new text, then replace it again.`)
      : make("A doc with this name already exists", `${nothing} Choose another title, or replace the existing doc.`);
  }
  if (status === 413) return make("The doc is too large", `${nothing} The server accepts up to 2 MB of Markdown. Split the doc and upload the parts.`);
  if (status === 400 || status === 422) return make("The doc was not accepted", `${nothing} Check that the file is Markdown text and that the title is filled in.`);
  if (status === 429) return make("Too many requests", `${nothing} Wait a moment and try again.`);
  if (status >= 500) return make(action === "save" ? "The server could not save the doc" : "The server could not delete the doc", `${nothing} Try again, or send the request ID to the platform team.`);
  return make(action === "save" ? "The doc was not saved" : "The doc was not deleted", nothing);
}
