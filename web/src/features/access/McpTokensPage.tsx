import { PersonalTokens } from "@/components/app/PersonalTokens";
import type { PageProps } from "@/routes/props";

// Personal MCP tokens (route /me/tokens): the tokens a person made to let their editor's MCP client act as them. The list, making a
// token (shown once, kept in component state only), revoking, and the mode that is not switched on are the shared component's
// (src/components/app/PersonalTokens.tsx, also mounted by the Admin MCP section); this page gives it its h1 and nothing else.
//
// The hub answers the list with 404 when personal-token mode is off. The account menu hides its entry for that (it asks the same
// list), and a direct link lands here and gets the component's empty state, never an error.
export default function McpTokensPage(_props: PageProps<"mcp-tokens">) {
  return (
    <div className="flex flex-col">
      <h1 className="px-6 pt-6 text-xl font-semibold tracking-tight">Personal MCP tokens</h1>
      <div className="max-w-[760px]">
        <PersonalTokens heading="Your tokens" />
      </div>
    </div>
  );
}
