export function serializeJson(value: unknown): {
  text: string;
  error?: string;
} {
  try {
    return { text: jsonText(value) };
  } catch {
    return {
      text: "",
      error: "Value could not be serialized. Copy individual fields instead.",
    };
  }
}

export function jsonText(value: unknown): string {
  const ancestors: object[] = [];
  return (
    JSON.stringify(
      value,
      function (this: unknown, _key, child: unknown) {
        if (typeof child === "bigint") return String(child);
        if (child !== null && typeof child === "object") {
          while (ancestors.length && ancestors.at(-1) !== this) ancestors.pop();
          if (ancestors.includes(child)) return "[Circular]";
          ancestors.push(child);
        }
        return child;
      },
      2
    ) ?? String(value)
  );
}
