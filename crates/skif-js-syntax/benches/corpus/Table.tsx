import React, { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import clsx from "clsx";
import { Row, type RowProps } from "./Row";
import styles from "./Table.module.css";
import type { Item } from "./store";

export interface Column<T> {
  key: keyof T & string;
  header: ReactNode;
  width?: number;
  render?: (value: T[keyof T], row: T) => ReactNode;
  sortable?: boolean;
}

interface TableProps<T extends { id: string }> {
  rows: readonly T[];
  columns: ReadonlyArray<Column<T>>;
  onSelect?: (row: T) => void;
  emptyMessage?: ReactNode;
  className?: string;
}

type SortState<T> = { key: keyof T & string; direction: "asc" | "desc" } | null;

function compare<T>(a: T, b: T, key: keyof T, direction: "asc" | "desc"): number {
  const left = a[key];
  const right = b[key];
  const result = left < right ? -1 : left > right ? 1 : 0;
  return direction === "asc" ? result : -result;
}

export function Table<T extends { id: string }>({
  rows,
  columns,
  onSelect,
  emptyMessage = "Nothing to show",
  className,
}: TableProps<T>) {
  const [sort, setSort] = useState<SortState<T>>(null);
  const [selected, setSelected] = useState<string | undefined>();

  const sorted = useMemo(() => {
    if (!sort) return rows;
    return [...rows].sort((a, b) => compare(a, b, sort.key, sort.direction));
  }, [rows, sort]);

  const toggle = useCallback((key: keyof T & string) => {
    setSort((current) =>
      current?.key === key
        ? { key, direction: current.direction === "asc" ? "desc" : "asc" }
        : { key, direction: "asc" },
    );
  }, []);

  useEffect(() => {
    if (selected && !rows.some((row) => row.id === selected)) {
      setSelected(undefined);
    }
  }, [rows, selected]);

  if (sorted.length === 0) {
    return <p className={styles.empty}>{emptyMessage}</p>;
  }

  return (
    <table className={clsx(styles.table, className)}>
      <thead>
        <tr>
          {columns.map((column) => (
            <th
              key={column.key}
              style={{ width: column.width }}
              onClick={column.sortable ? () => toggle(column.key) : undefined}
              aria-sort={sort?.key === column.key ? (sort.direction === "asc" ? "ascending" : "descending") : "none"}
            >
              {column.header}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {sorted.map((row) => (
          <Row<T>
            key={row.id}
            row={row}
            columns={columns}
            active={row.id === selected}
            onClick={() => {
              setSelected(row.id);
              onSelect?.(row);
            }}
          />
        ))}
      </tbody>
    </table>
  );
}

export const ItemTable = (props: Omit<TableProps<Item>, "columns">) => (
  <Table<Item>
    {...props}
    columns={[
      { key: "title", header: <strong>Title</strong>, sortable: true },
      { key: "createdAt", header: "Created", render: (value) => String(value) },
    ]}
  />
);

export type { RowProps };
