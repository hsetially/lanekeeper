// How a rendered doc looks. The page the server sends has no `class` and no `style` (its allowlist forbids both), so the
// look comes from the element names, with Tailwind's arbitrary variants on the wrapper. Sizes follow the Search page of the
// handoff: 14/22 text, 16/22 headings, a bordered table, a muted code block. A heading that a link or the table of contents
// named carries `data-lk-target` (the reader sets it), because `:target` does not follow a route change made with pushState.

export const PROSE = [
  "max-w-[760px] text-[14px] leading-[22px] text-pretty text-foreground",
  "[&>*+*]:mt-3",
  "[&_h1]:text-xl [&_h1]:font-semibold [&_h1]:leading-7 [&_h1]:tracking-tight",
  "[&_h2]:-ml-2 [&_h2]:mt-5 [&_h2]:scroll-mt-4 [&_h2]:rounded-sm [&_h2]:px-2 [&_h2]:py-1 [&_h2]:text-base [&_h2]:font-semibold [&_h2]:leading-[22px]",
  "[&_h3]:-ml-2 [&_h3]:mt-4 [&_h3]:scroll-mt-4 [&_h3]:rounded-sm [&_h3]:px-2 [&_h3]:py-0.5 [&_h3]:text-[15px] [&_h3]:font-semibold",
  "[&_h4]:-ml-2 [&_h4]:mt-3 [&_h4]:scroll-mt-4 [&_h4]:rounded-sm [&_h4]:px-2 [&_h4]:text-sm [&_h4]:font-semibold",
  "[&_h5]:scroll-mt-4 [&_h5]:font-semibold [&_h6]:scroll-mt-4 [&_h6]:font-semibold",
  "[&_[data-lk-target]]:bg-selection",
  "[&_ul]:list-disc [&_ul]:pl-5 [&_ol]:list-decimal [&_ol]:pl-5 [&_li+li]:mt-1",
  "[&_blockquote]:border-l-2 [&_blockquote]:border-border [&_blockquote]:pl-3 [&_blockquote]:text-muted-foreground",
  "[&_hr]:border-border",
  "[&_code]:rounded-[3px] [&_code]:border [&_code]:border-border [&_code]:bg-muted [&_code]:px-[3px] [&_code]:font-mono [&_code]:text-xs",
  "[&_pre]:overflow-auto [&_pre]:rounded-md [&_pre]:border [&_pre]:border-border [&_pre]:bg-muted [&_pre]:px-3 [&_pre]:py-2 [&_pre]:text-xs [&_pre]:leading-[18px]",
  "[&_pre_code]:border-0 [&_pre_code]:bg-transparent [&_pre_code]:p-0",
  "[&_table]:block [&_table]:max-w-full [&_table]:overflow-auto [&_table]:rounded-md [&_table]:border [&_table]:border-border [&_table]:text-[13px]",
  "[&_th]:border-b [&_th]:border-border [&_th]:bg-muted [&_th]:px-3 [&_th]:py-1.5 [&_th]:text-left [&_th]:text-xs [&_th]:font-medium [&_th]:text-muted-foreground",
  "[&_td]:border-b [&_td]:border-border [&_td]:px-3 [&_td]:py-1.5 [&_td]:align-top",
  "[&_a]:underline-offset-2 [&_a:hover]:underline",
].join(" ");
