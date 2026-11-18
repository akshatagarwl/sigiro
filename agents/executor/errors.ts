import { Schema } from "effect";

/**
 * The single failure type for every executor operation. `operation` names the
 * step that failed and `detail` carries the reason, so the message the
 * coordinator eventually sees still reads `"<operation>: <detail>"`.
 */
export class ExecutorError extends Schema.TaggedError<ExecutorError>()("ExecutorError", {
  operation: Schema.String,
  detail: Schema.String,
  cause: Schema.optionalKey(Schema.Defect()),
}) {
  /**
   * Kept positional so the existing call sites, which read as prose
   * (`new ExecutorError("read job", String(cause), cause)`), survive the move
   * to `Schema.TaggedError`.
   */
  constructor(operation: string, detail: string, cause?: unknown) {
    super(cause === undefined ? { operation, detail } : { operation, detail, cause });
  }

  override get message(): string {
    return `${this.operation}: ${this.detail}`;
  }
}

export class RequestConflict extends Schema.TaggedError<RequestConflict>()("RequestConflict", {
  executionId: Schema.String,
}) {
  constructor(executionId: string) {
    super({ executionId });
  }

  override get message(): string {
    return `execution ${this.executionId} already exists with different input`;
  }
}
