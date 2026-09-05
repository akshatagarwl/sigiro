import { Schema } from "effect";

export class BridgeError extends Schema.TaggedError<BridgeError>()("BridgeError", {
  operation: Schema.String,
  detail: Schema.String,
  retryable: Schema.Boolean,
}) {
  override get message(): string { return `${this.operation}: ${this.detail}`; }
}
