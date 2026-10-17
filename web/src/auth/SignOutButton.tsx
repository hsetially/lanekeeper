import { Banner } from "@/components/app/Banner";
import { Button } from "@/components/ui/button";
import { useSignOut } from "./signout";

export function SignOutButton() {
  const { signOut, pending, problem } = useSignOut();
  return (
    <>
      <Button
        variant="secondary"
        loading={pending}
        onClick={() => {
          void signOut();
        }}
      >
        Sign out
      </Button>
      {problem ? <Banner variant="danger" title="Could not sign out">{problem.title}</Banner> : null}
    </>
  );
}
