import { Fragment } from "react";
import { useRouterState } from "@tanstack/react-router";
import { SafeText } from "@/components/app/SafeText";
import { ChevronRight } from "@/icons";
import { cn } from "@/lib/utils";
import { isPlainClick, useNavigateTo } from "@/routes/AppLink";
import { crumbsFor, type Crumb } from "./crumbs";

// The breadcrumb of the header (the Shell). The last crumb is the page (aria-current) and the one that shrinks, so at a
// narrow width the path is cut before anything else in the header is; the crumbs before it shrink too, to an ellipsis, but only
// once it has nothing left to give (its share of what is lacking is a thousand times theirs). Paths and names are the file system's and the
// registry's, so they are drawn through SafeText.

// The mono face and its size belong to the element that holds the text (the link, the span, the item), not to a span inside it:
// the design sets them there, and a smaller inline box inside a larger line moves the text by half a pixel. `text-[12px]` sets
// the size only: `text-xs` also sets a 16 px line height, and the design leaves the line height of the page (18 px).
const mono = (crumb: Crumb): string | undefined => (crumb.mono ? "font-mono text-[12px]" : undefined);

function CrumbText({ crumb }: { crumb: Crumb }) {
  return <SafeText text={crumb.label} />;
}

export function Breadcrumb() {
  const match = useRouterState({
    select: (s) => {
      const last = s.matches.at(-1);
      return { id: last?.staticData.routeId, params: (last?.params ?? {}) as Record<string, string>, search: (last?.search ?? {}) as Record<string, string> };
    },
  });
  const crumbs = crumbsFor(match.id, match.params, match.search);
  return (
    <nav aria-label="Breadcrumb" className="min-w-0 flex-1 overflow-hidden">
      <ol className="m-0 flex list-none items-center gap-1 p-0 whitespace-nowrap">
        {crumbs.map((crumb, index) => {
          const last = index === crumbs.length - 1;
          return (
            <Fragment key={`${index}:${crumb.label}`}>
              {index > 0 ? (
                <li aria-hidden="true" className="flex flex-none">
                  <ChevronRight className="size-3 text-muted-foreground" />
                </li>
              ) : null}
              {last ? (
                <li aria-current="page" className={cn("min-w-0 shrink-[1000] overflow-hidden font-medium text-ellipsis", mono(crumb))}>
                  <CrumbText crumb={crumb} />
                </li>
              ) : (
                <li className="min-w-0">
                  {crumb.href ? (
                    <BreadcrumbLink crumb={crumb} />
                  ) : (
                    <span className={cn("block truncate text-muted-foreground", mono(crumb))}>
                      <CrumbText crumb={crumb} />
                    </span>
                  )}
                </li>
              )}
            </Fragment>
          );
        })}
      </ol>
    </nav>
  );
}

function BreadcrumbLink({ crumb }: { crumb: Crumb }) {
  const navigate = useNavigateTo();
  return (
    <a
      href={crumb.href}
      onClick={(event) => {
        if (crumb.href && isPlainClick(event)) {
          event.preventDefault();
          navigate(crumb.href);
        }
      }}
      className={cn("block truncate text-muted-foreground no-underline hover:text-foreground focus-visible:-outline-offset-2", mono(crumb))}
    >
      <CrumbText crumb={crumb} />
    </a>
  );
}
