import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { OsLoginUser } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { visibleText } from "@/components/app/safe-text";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { NativeSelect } from "@/components/ui/native-select";
import { adminMutations } from "@/features/admin/mutations";
import { adminQueries } from "@/features/admin/queries";
import { omitKey } from "@/features/admin/validate";
import { adminWriteError } from "@/features/admin/write-error";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { checkOsLogin, pathSegment, userOptionText } from "./os-login-model";

// One OS Login mapping: a login name as the VM records it, and the Lanekeeper user it stands for. The name is the mapping's key, so an
// existing one cannot be renamed (delete it and add another). Every user is offered, with the state of those who are not active: the
// registry attributes by an admin's mapping without asking the user's state, because a user who has since been disabled still made
// the change; the hub may still refuse a choice (422), and the dialog then keeps what was typed.

type Errors = { login?: string; user?: string };

const CONFLICT = "Someone else changed this mapping at the same time. Reload the list, check what is there now and try again.";

export function OsLoginDialog({ mapping, existing, onClose, onSaved }: { mapping: OsLoginUser | null; existing: ReadonlySet<string>; onClose: () => void; onSaved: (login: string) => void }) {
  const client = useQueryClient();
  const save = useMutation(adminMutations.putOsLoginUser(client));
  const users = useCursorPages({ ...adminQueries.users(), autoLoad: true });
  const editing = mapping !== null;
  const [login, setLogin] = useState(mapping?.os_login ?? "");
  const [user, setUser] = useState(mapping?.user ?? "");
  const [errors, setErrors] = useState<Errors>({});

  const options = useMemo(() => {
    const sorted = [...users.items].sort((a, b) => a.display_name.localeCompare(b.display_name) || a.id.localeCompare(b.id));
    const known = sorted.some((u) => u.id === mapping?.user);
    return { sorted, missing: mapping !== null && !known ? mapping.user : null };
  }, [users.items, mapping]);

  const submit = (): void => {
    const found: Errors = {};
    const name = login.trim();
    if (!editing) {
      const bad = checkOsLogin(name);
      if (bad !== null) found.login = bad;
      else if (existing.has(name)) found.login = "This login name is mapped already. Use Edit on its row to change the user.";
    }
    if (user === "") found.user = "Choose the user this name stands for";
    setErrors(found);
    if (Object.keys(found).length > 0) return;
    const key = editing ? mapping.os_login : name;
    save.mutate(
      { login: pathSegment(key), body: { user } },
      {
        onSuccess: () => {
          onSaved(key);
          onClose();
        },
      },
    );
  };

  const loading = users.status === "pending";
  const failed = users.status === "error";
  return (
    <ConsequenceDialog
      initialFocus="first-field"
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title={editing ? `Edit mapping ${visibleText(mapping.os_login)}` : "Add an OS Login mapping"}
      consequence={
        editing
          ? "Saving changes who edits recorded under this login name are attributed to, from now on."
          : "Edits recorded under this login name are attributed to the user chosen, from now on, instead of being matched by email."
      }
      actionLabel="Save mapping"
      workingLabel="Saving…"
      working={save.isPending}
      error={save.error === null ? null : adminWriteError(save.error, "The mapping could not be saved", { 409: CONFLICT })}
      onConfirm={submit}
    >
      <Field invalid={errors.login !== undefined}>
        <FieldLabel>OS Login username</FieldLabel>
        <FieldControl>
          <Input value={login} readOnly={editing} autoComplete="off" spellCheck={false} placeholder="dana_okoye_corp_example" className="font-mono" onChange={(event) => setLogin(event.target.value)} />
        </FieldControl>
        <FieldError>{errors.login}</FieldError>
        <FieldDescription>{editing ? "The name cannot change. Delete the mapping and add another to rename it." : "The name the VM records for the person's session, such as dana_okoye_corp_example for dana.okoye@corp.example."}</FieldDescription>
      </Field>
      <Field invalid={errors.user !== undefined}>
        <FieldLabel>Lanekeeper user</FieldLabel>
        <FieldControl>
          <NativeSelect
            value={user}
            disabled={loading || failed}
            onChange={(event) => {
              setUser(event.target.value);
              setErrors(omitKey(errors, "user"));
            }}
          >
            <option value="">{loading ? "Loading users…" : failed ? "Users could not be loaded" : "Choose a user"}</option>
            {options.missing === null ? null : <option value={options.missing}>{`Unknown user ${options.missing}`}</option>}
            {options.sorted.map((u) => (
              <option key={u.id} value={u.id}>
                {userOptionText(u)}
              </option>
            ))}
          </NativeSelect>
        </FieldControl>
        <FieldError>{errors.user}</FieldError>
        {failed ? <FieldDescription>The list of users could not be read, so no user can be chosen now. Close this and try again.</FieldDescription> : null}
      </Field>
    </ConsequenceDialog>
  );
}
