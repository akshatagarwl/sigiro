"use client";

import { Collapsible } from "@base-ui/react/collapsible";
import { useState } from "react";

import { ActionMenu, Button, Input } from "./primitives";
import { serializeJson } from "./value";

import styles from "./observability.module.css";

export function AttributeTree({
  value,
  onCopy,
}: {
  value: unknown;
  onCopy: (value: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [copyError, setCopyError] = useState("");
  const copyJson = (data: unknown) => {
    const result = serializeJson(data);
    if (result.error) setCopyError(result.error);
    else {
      setCopyError("");
      onCopy(result.text);
    }
  };
  return (
    <div className={styles.attributes}>
      <Input
        aria-label="Find event attributes"
        placeholder="Find a key or value…"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      <TreeValue
        name="attributes"
        value={value}
        depth={0}
        query={query.toLowerCase()}
        copyJson={copyJson}
        onCopy={onCopy}
      />
      <p role="status">{copyError}</p>
    </div>
  );
}

function TreeValue({
  name,
  value,
  depth,
  query,
  onCopy,
  copyJson,
}: {
  name: string;
  value: unknown;
  depth: number;
  query: string;
  copyJson: (value: unknown) => void;
  onCopy: (value: string) => void;
}) {
  const [open, setOpen] = useState(depth === 0);
  if (value !== null && typeof value === "object" && depth < 8) {
    const all = Object.entries(value);
    const matches = all.filter(
      ([key, child]) =>
        !query ||
        key.toLowerCase().includes(query) ||
        serializeJson(child).text.toLowerCase().includes(query)
    );
    return (
      <Collapsible.Root open={open} onOpenChange={setOpen}>
        <div className={styles.attributeHeader}>
          <Collapsible.Trigger render={<Button />}>
            <span aria-hidden="true">{open ? "▾" : "▸"}</span> {name}{" "}
            <span className={styles.muted}>
              {all.length} {Array.isArray(value) ? "items" : "keys"}
            </span>
          </Collapsible.Trigger>
          <ActionMenu
            label={`Actions for ${name}`}
            actions={[{ label: "Copy JSON", onClick: () => copyJson(value) }]}
          >
            ···
          </ActionMenu>
        </div>
        <Collapsible.Panel className={styles.attributeChildren}>
          {open && (
            <>
              {matches.slice(0, 200).map(([key, child]) => (
                <TreeValue
                  key={key}
                  name={key}
                  value={child}
                  depth={depth + 1}
                  query={query}
                  copyJson={copyJson}
                  onCopy={onCopy}
                />
              ))}
              {matches.length > 200 && (
                <p className={styles.muted}>
                  Showing 200 of {matches.length} keys. Copy JSON for the full
                  value.
                </p>
              )}
              {!matches.length && (
                <p className={styles.muted}>
                  {query
                    ? "No matching attributes."
                    : "No attributes recorded."}
                </p>
              )}
            </>
          )}
        </Collapsible.Panel>
      </Collapsible.Root>
    );
  }
  const result = serializeJson(value);
  const text = result.error ?? result.text;
  return (
    <div className={styles.attributeLeaf}>
      <span className={styles.attributeKey}>{name}</span>
      <code>
        {text.slice(0, 10_000)}
        {text.length > 10_000 && "… (preview; copy for full value)"}
      </code>
      <ActionMenu
        label={`Actions for ${name}`}
        actions={[
          {
            label: "Copy value",
            onClick: () =>
              typeof value === "string" ? onCopy(value) : copyJson(value),
          },
        ]}
      >
        ···
      </ActionMenu>
    </div>
  );
}
