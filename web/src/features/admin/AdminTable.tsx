import { DataTable, type Column, type TableFooter } from "@/components/app/virtual/DataTable";

// Every Admin list is the shared DataTable (a virtualised grid with 32 px rows), in a box that is as tall as its rows up to a limit, so
// a list of five rules is not a tall empty frame and a list of hundreds scrolls inside its own box. The box carries the height as a
// style property (a CSSOM write, which the CSP allows); the table fills it.

const HEADER = 32;
const ROW = 32;
const FOOTER = 36;
/** Rows beyond this scroll inside the box. */
const MAX_HEIGHT = 480;

export function tableHeight(rows: number, footer: boolean): number {
  return Math.min(MAX_HEIGHT, HEADER + rows * ROW + (footer ? FOOTER : 0) + 2);
}

type Props<Row> = {
  /** The accessible name of the grid. */
  label: string;
  rows: readonly Row[];
  columns: readonly Column<Row>[];
  rowKey: (row: Row, index: number) => string;
  footer?: TableFooter;
};

export function AdminTable<Row>({ label, rows, columns, rowKey, footer }: Props<Row>) {
  return (
    <div style={{ height: tableHeight(rows.length, footer !== undefined) }} className="flex min-h-0 flex-col overflow-hidden rounded-md border border-border bg-card">
      <DataTable label={label} rows={rows} columns={columns} rowKey={rowKey} className="h-full" {...(footer ? { footer } : {})} />
    </div>
  );
}
