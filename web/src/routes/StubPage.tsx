import { EmptyState } from "@/components/app/EmptyState";

/** What a route shows until its slice's screen replaces it: the route's title as the h1, and the shared empty state. */
export function StubPage({ title }: { title: string }) {
  return (
    <div className="flex flex-col">
      <h1 className="px-6 pt-4 text-xl font-semibold">{title}</h1>
      <EmptyState title="This screen is built in a later task" className="pt-16" />
    </div>
  );
}
