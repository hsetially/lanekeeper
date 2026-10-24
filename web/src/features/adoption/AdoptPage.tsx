import { StubPage } from "@/routes/StubPage";

// Stub made by the app shell (T4, S0) so that the route resolves and the app builds before slice S6b lands. S6b REPLACES THIS FILE
// with the real screen: a default export that takes `PageProps<"adopt">` (src/routes/props.ts), reads nothing from the URL by
// itself, and renders no <main> (the shell provides it).
export default function AdoptPage() {
  return <StubPage title="Adopt this swimlane" />;
}
