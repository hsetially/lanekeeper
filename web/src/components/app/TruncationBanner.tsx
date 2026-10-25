import { Banner } from "./Banner";

type TruncationBannerProps = {
  /** How many items are shown (the cap the server applied). */
  shown: number;
  /** What they are: "matches", "files", "findings". */
  noun: string;
  className?: string;
};

/**
 * A capped result set says so. The API reports only that the result was cut off, not how many there are, so this
 * shows the cap and never a total (design/DEVIATIONS.md: "shown of total" is not available).
 */
export function TruncationBanner({ shown, noun, className }: TruncationBannerProps) {
  return (
    <Banner variant="warning" title="Results cut off" {...(className ? { className } : {})}>
      Showing the first {shown.toLocaleString("en-US")} {noun}. Narrow the query to see the rest.
    </Banner>
  );
}
