import { useRouterState } from "@tanstack/react-router";
import { routeById } from "./table";

/** What a route shows until its screen is built (T4 replaces the loader in src/routes/pages.ts, not this file). */
export default function Placeholder() {
  const id = useRouterState({ select: (s) => s.matches.at(-1)?.staticData.routeId });
  const title = id === undefined ? "Lanekeeper" : routeById(id).title;
  return (
    <main className="flex flex-col gap-3 p-6">
      <h1 className="text-lg font-semibold">{title}</h1>
      <p className="text-muted-foreground">This screen is built in a later task.</p>
    </main>
  );
}
