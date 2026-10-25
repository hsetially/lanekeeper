import type { ServiceState } from "@/api/generated/model";

/** The services whose name or namespace contains the text, in any case. Plain text, never a pattern. */
export function filterServices(services: readonly ServiceState[], text: string): ServiceState[] {
  const needle = text.trim().toLowerCase();
  if (needle === "") return [...services];
  return services.filter((s) => s.service.name.toLowerCase().includes(needle) || s.service.namespace.toLowerCase().includes(needle));
}
