export type EventRecord = {
  id: string;
  timestamp: Date;
  service: string;
  severity: string;
  category: string;
  message: string;
  attributes: unknown;
  traceId?: string;
  spanId?: string;
};

export type EventField = "service" | "severity" | "message" | "traceId";
export type EventFilter = (
  field: EventField,
  value: string,
  action: "include" | "exclude"
) => void;
export type EventColumn = "timestamp" | "severity" | "service" | "message";
