"use client";

import { Button as RowButton } from "@base-ui/react/button";
import { Dialog } from "@base-ui/react/dialog";
import { Menu } from "@base-ui/react/menu";
import { Tabs } from "@base-ui/react/tabs";
import { useVirtualizer } from "@tanstack/react-virtual";
import * as React from "react";
import { Group, Panel, Separator } from "react-resizable-panels";

import { AttributeTree } from "./attribute-tree";
import { ActionMenu, Button, Input } from "./primitives";
import type { EventColumn, EventFilter, EventRecord } from "./types";
import { serializeJson } from "./value";

import styles from "./observability.module.css";

const COLUMN_LABELS: Record<EventColumn, string> = {
  timestamp: "Time",
  severity: "Level",
  service: "Service",
  message: "Message",
};
const DEFAULT_COLUMNS: EventColumn[] = [
  "timestamp",
  "severity",
  "service",
  "message",
];

export function EventExplorer({
  events,
  selectedId,
  onSelect,
  onFilter,
  traceLink,
  pagination,
}: {
  pagination?: { loading: boolean; hasMore: boolean; onLoadMore: () => void };
  events: EventRecord[];
  selectedId?: string;
  onSelect: (id: string | undefined) => void;
  onFilter?: EventFilter;
  traceLink?: (event: EventRecord) => React.ReactNode;
}) {
  const inspectorId = React.useId();
  const [find, setFind] = React.useState("");
  const [wrap, setWrap] = React.useState(false);
  const [comfortable, setComfortable] = React.useState(false);
  const [columns, setColumns] = React.useState(DEFAULT_COLUMNS);
  const [notice, setNotice] = React.useState("");
  const [mobile, setMobile] = React.useState(false);
  const scrollRef = React.useRef<HTMLDivElement>(null);
  const headerRef = React.useRef<HTMLDivElement>(null);
  const [activeId, setActiveId] = React.useState<string>();
  const returnFocus = React.useRef<HTMLElement | null>(null);
  React.useEffect(() => {
    const media = window.matchMedia("(max-width: 767px)");
    const update = () => setMobile(media.matches);
    update();
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  const rows = React.useMemo(() => {
    const needle = find.trim().toLowerCase();
    return events.filter(
      (event) =>
        !needle ||
        `${event.service} ${event.severity} ${event.message}`
          .toLowerCase()
          .includes(needle)
    );
  }, [events, find]);
  const selected = events.find((event) => event.id === selectedId);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => (comfortable ? 48 : 34),
    getItemKey: (index) => rows[index].id,
    overscan: 8,
  });
  React.useEffect(() => {
    virtualizer.measure();
  }, [wrap, comfortable, columns, mobile, selectedId, virtualizer]);
  const copy = (value: string) => {
    if (!navigator.clipboard) {
      setNotice("Clipboard unavailable. Full value was not copied.");
      return;
    }
    void navigator.clipboard.writeText(value).then(
      () => setNotice("Copied"),
      () =>
        setNotice(
          "Copy failed. Full value was not copied; check clipboard permission."
        )
    );
  };
  const returnFocusTarget = React.useCallback(() => {
    if (returnFocus.current?.isConnected) return returnFocus.current;
    const index = rows.findIndex(
      (row) => row.id === returnFocus.current?.dataset.eventRow
    );
    return (
      scrollRef.current?.querySelector<HTMLElement>(
        `[data-event-focus="${index}"]`
      ) ?? scrollRef.current
    );
  }, [rows]);
  const previouslySelected = React.useRef(Boolean(selectedId));
  React.useEffect(() => {
    const wasSelected = previouslySelected.current;
    previouslySelected.current = Boolean(selectedId);
    if (selectedId || !wasSelected) return;
    const frame = requestAnimationFrame(() =>
      returnFocusTarget()?.focus({ preventScroll: true })
    );
    return () => cancelAnimationFrame(frame);
  }, [selectedId, returnFocusTarget]);
  const close = () => {
    onSelect(undefined);
  };
  const open = (event: EventRecord, target: HTMLElement) => {
    returnFocus.current = target;
    setActiveId(event.id);
    onSelect(selectedId === event.id ? undefined : event.id);
  };
  const navigate = (index: number) => {
    const row = rows[Math.max(0, Math.min(rows.length - 1, index))];
    if (!row) return;
    setActiveId(row.id);
    virtualizer.scrollToIndex(rows.indexOf(row), { align: "auto" });
    requestAnimationFrame(() =>
      scrollRef.current
        ?.querySelector<HTMLElement>(
          `[data-event-focus="${rows.indexOf(row)}"]`
        )
        ?.focus()
    );
  };
  const grid = columns
    .map(
      (column) =>
        ({
          timestamp: "155px",
          severity: "80px",
          service: "140px",
          message: "minmax(200px, 1fr)",
        })[column]
    )
    .join(" ");
  const inspector = selected ? (
    <Inspector
      event={selected}
      onClose={close}
      onCopy={copy}
      onFilter={onFilter}
      traceLink={traceLink}
      closeControl={
        mobile ? (
          <Dialog.Close render={<Button aria-label="Close event inspector" />}>
            ×
          </Dialog.Close>
        ) : undefined
      }
    />
  ) : null;
  const content = (
    <div className={styles.tablePane}>
      <div className={styles.resultsToolbar}>
        <div className={styles.resultsHeading}>
          <strong>Events</strong>
          <span className={styles.muted}>
            {rows.length.toLocaleString()} / {events.length.toLocaleString()}{" "}
            loaded
          </span>
        </div>
        <div className={styles.tableActions}>
          <Input
            aria-label="Find in loaded events"
            placeholder="Find in loaded events…"
            value={find}
            onChange={(event) => setFind(event.target.value)}
          />
          <Button aria-pressed={wrap} onClick={() => setWrap(!wrap)}>
            Wrap
          </Button>
          <Button
            aria-pressed={comfortable}
            onClick={() => setComfortable(!comfortable)}
          >
            Comfortable
          </Button>
          <Menu.Root>
            <Menu.Trigger render={<Button />}>Columns</Menu.Trigger>
            <Menu.Portal>
              <Menu.Positioner sideOffset={6} className={styles.positioner}>
                <Menu.Popup className={styles.menu}>
                  {DEFAULT_COLUMNS.map((column) => (
                    <Menu.CheckboxItem
                      key={column}
                      className={styles.menuItem}
                      checked={columns.includes(column)}
                      disabled={columns.length === 1 && columns[0] === column}
                      onCheckedChange={(checked) =>
                        setColumns(
                          DEFAULT_COLUMNS.filter((key) =>
                            key === column ? checked : columns.includes(key)
                          )
                        )
                      }
                    >
                      <Menu.CheckboxItemIndicator>
                        ✓{" "}
                      </Menu.CheckboxItemIndicator>
                      {COLUMN_LABELS[column]}
                    </Menu.CheckboxItem>
                  ))}
                </Menu.Popup>
              </Menu.Positioner>
            </Menu.Portal>
          </Menu.Root>
        </div>
      </div>
      <div
        ref={headerRef}
        className={styles.columnHeader}
        aria-hidden="true"
        style={{ gridTemplateColumns: grid }}
      >
        {columns.map((column) => (
          <span key={column}>{COLUMN_LABELS[column]}</span>
        ))}
      </div>
      <div
        ref={scrollRef}
        onScroll={(event) => {
          if (headerRef.current)
            headerRef.current.scrollLeft = event.currentTarget.scrollLeft;
        }}
        className={styles.resultsScroll}
        role="region"
        aria-label="Log events. Arrow keys move focus; Enter opens event."
        tabIndex={0}
        onKeyDown={(event) => {
          if ((event.target as HTMLElement).closest('[role="menu"]')) return;
          const index = rows.findIndex((row) => row.id === activeId);
          if (
            event.key === "ArrowDown" ||
            event.key === "ArrowUp" ||
            event.key === "Home" ||
            event.key === "End"
          ) {
            event.preventDefault();
            navigate(
              event.key === "Home"
                ? 0
                : event.key === "End"
                  ? rows.length - 1
                  : index + (event.key === "ArrowDown" ? 1 : -1)
            );
          }
        }}
      >
        {rows.length ? (
          <div
            role="list"
            aria-label="Loaded log events"
            style={{ height: virtualizer.getTotalSize(), position: "relative" }}
          >
            {virtualizer.getVirtualItems().map((item) => {
              const row = rows[item.index];
              return (
                <div
                  key={row.id}
                  data-index={item.index}
                  ref={virtualizer.measureElement}
                  role="listitem"
                  aria-posinset={item.index + 1}
                  aria-setsize={rows.length}
                  className={styles.virtualRow}
                  style={{ transform: `translateY(${item.start}px)` }}
                >
                  <RowButton
                    type="button"
                    data-event-focus={item.index}
                    data-event-row={row.id}
                    aria-expanded={row.id === selectedId}
                    aria-controls={
                      row.id === selectedId ? inspectorId : undefined
                    }
                    data-selected={row.id === selectedId || undefined}
                    data-comfortable={comfortable || undefined}
                    data-wrap={wrap || undefined}
                    className={styles.eventRow}
                    style={{ gridTemplateColumns: grid }}
                    tabIndex={row.id === (activeId ?? rows[0]?.id) ? 0 : -1}
                    onFocus={() => setActiveId(row.id)}
                    onClick={(event) => open(row, event.currentTarget)}
                  >
                    {columns.map((column) => (
                      <span
                        key={column}
                        data-column={column}
                        className={
                          column === "severity" ? styles.severity : undefined
                        }
                        data-level={
                          column === "severity" ? row.category : undefined
                        }
                      >
                        {column === "timestamp" ? (
                          <time dateTime={row.timestamp.toISOString()}>
                            {row.timestamp.toLocaleTimeString(undefined, {
                              hour12: false,
                              hour: "2-digit",
                              minute: "2-digit",
                              second: "2-digit",
                              fractionalSecondDigits: 3,
                            })}
                          </time>
                        ) : column === "severity" ? (
                          row.severity || row.category
                        ) : column === "service" ? (
                          row.service
                        ) : (
                          row.message.slice(0, 500)
                        )}
                      </span>
                    ))}
                  </RowButton>
                </div>
              );
            })}
          </div>
        ) : (
          <div className={styles.empty}>
            <strong>No loaded events match</strong>
            <p>
              Clear find, adjust the server filters or load more events below.
            </p>
          </div>
        )}
      </div>
      {pagination && (
        <div className={styles.pagination}>
          {pagination.hasMore || pagination.loading ? (
            <Button
              disabled={pagination.loading}
              onClick={pagination.onLoadMore}
            >
              {pagination.loading ? "Loading events…" : "Load more events"}
            </Button>
          ) : (
            <span className={styles.muted}>No more events in this query.</span>
          )}
        </div>
      )}
      <div className={styles.tableFooter}>
        Find searches loaded events only. Rows preview the first 500 characters;
        open an event for its full message.
      </div>
    </div>
  );
  return (
    <section
      className={styles.explorer}
      aria-label="Structured log explorer"
      aria-busy={pagination?.loading}
      onKeyDown={(event) => {
        if (
          event.key === "Escape" &&
          selectedId &&
          !event.defaultPrevented &&
          !mobile &&
          !(event.target as HTMLElement).closest('[role="menu"]')
        )
          close();
      }}
    >
      <div role="status" className={styles.notice}>
        {notice}
      </div>
      {!mobile ? (
        <Group orientation="horizontal" className={styles.panelGroup}>
          <Panel id="event-list" minSize="35%" defaultSize="65%">
            {content}
          </Panel>
          {selected && (
            <Separator
              aria-label="Resize event list and inspector"
              className={styles.resizeHandle}
            />
          )}
          {selected && (
            <Panel id="event-inspector" minSize="260px" defaultSize="35%">
              <aside
                id={inspectorId}
                aria-label="Event inspector"
                className={styles.inspector}
              >
                {inspector}
              </aside>
            </Panel>
          )}
        </Group>
      ) : (
        content
      )}
      {selectedId && !selected && (
        <p role="status" className={styles.tableFooter}>
          Selected event is not in loaded results. Load more or{" "}
          <Button onClick={close}>clear selection</Button>.
        </p>
      )}
      {mobile && (
        <Dialog.Root
          open={Boolean(selected)}
          onOpenChange={(open) => {
            if (!open) close();
          }}
        >
          <Dialog.Portal>
            <Dialog.Backdrop className={styles.backdrop} />
            <Dialog.Popup
              className={styles.mobileInspector}
              id={inspectorId}
              finalFocus={returnFocusTarget}
            >
              <Dialog.Title className={styles.srOnly}>
                Event details
              </Dialog.Title>
              <Dialog.Description className={styles.srOnly}>
                Full log message, structured attributes and correlation links.
              </Dialog.Description>
              {inspector}
            </Dialog.Popup>
          </Dialog.Portal>
        </Dialog.Root>
      )}
    </section>
  );
}

function Inspector({
  event,
  onClose,
  onCopy,
  onFilter,
  traceLink,
  closeControl,
}: {
  event: EventRecord;
  onClose: () => void;
  closeControl?: React.ReactNode;
  onCopy: (value: string) => void;
  onFilter?: EventFilter;
  traceLink?: (event: EventRecord) => React.ReactNode;
}) {
  const serialized = React.useMemo(() => serializeJson(event), [event]);
  const eventJson = serialized.text;
  return (
    <>
      <header className={styles.inspectorHeader}>
        <div>
          <span className={styles.eyebrow}>Event details</span>
          <h2>{event.service || "Unknown service"}</h2>
        </div>
        {closeControl ?? (
          <Button aria-label="Close event inspector" onClick={onClose}>
            ×
          </Button>
        )}
      </header>
      <div className={styles.metadata}>
        <span className={styles.severity} data-level={event.category}>
          {event.severity || event.category}
        </span>
        <time dateTime={event.timestamp.toISOString()}>
          {event.timestamp.toLocaleString()}
        </time>
      </div>
      <div className={styles.inspectorActions}>
        <Button onClick={() => onCopy(event.message)}>Copy message</Button>
        {onFilter && event.service && (
          <ActionMenu
            label="Filter service"
            actions={[
              {
                label: `Include ${event.service}`,
                onClick: () => onFilter("service", event.service, "include"),
              },
            ]}
          >
            Filter service
          </ActionMenu>
        )}
        {traceLink?.(event)}
      </div>
      <div className={styles.messageSection}>
        <h3>Message</h3>
        <pre tabIndex={0} className={styles.message}>
          {event.message.slice(0, 50_000)}
        </pre>
        {event.message.length > 50_000 && (
          <p className={styles.muted}>
            Showing 50,000 of {event.message.length.toLocaleString()}{" "}
            characters. Copy message preserves the full value.
          </p>
        )}
      </div>
      <Tabs.Root
        defaultValue="attributes"
        className={styles.tabs}
        key={event.id}
      >
        <Tabs.List className={styles.tabList} aria-label="Event fields">
          <Tabs.Tab className={styles.tab} value="attributes">
            Attributes
          </Tabs.Tab>
          <Tabs.Tab className={styles.tab} value="json">
            Event JSON
          </Tabs.Tab>
        </Tabs.List>
        <Tabs.Panel value="attributes">
          <AttributeTree value={event.attributes} onCopy={onCopy} />
        </Tabs.Panel>
        <Tabs.Panel value="json">
          <div className={styles.jsonActions}>
            <Button
              disabled={Boolean(serialized.error)}
              onClick={() => onCopy(eventJson)}
            >
              Copy event JSON
            </Button>
          </div>
          {serialized.error && <p role="alert">{serialized.error}</p>}
          <pre tabIndex={0} className={styles.json}>
            {eventJson.slice(0, 50_000)}
          </pre>
          {eventJson.length > 50_000 && (
            <p className={styles.muted}>
              JSON preview limited to 50,000 characters. Copy event JSON
              preserves the full value.
            </p>
          )}
        </Tabs.Panel>
      </Tabs.Root>
    </>
  );
}
