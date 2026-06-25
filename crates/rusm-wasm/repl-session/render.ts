// Turning a JavaScript value into the one-line string the REPL shows. Kept
// separate from the engine so it is trivially unit-testable.

/** Render an eval's return value the way a REPL would echo it. */
export function render(value: unknown): string {
  switch (typeof value) {
    case "bigint":
      return value.toString(); // pids are bigints — show them bare
    case "string":
      return JSON.stringify(value); // quoted, so "1" reads differently from 1
    case "function":
      return `[Function: ${(value as { name?: string }).name || "anonymous"}]`;
    case "symbol":
      return value.toString();
    case "object": {
      if (value === null) return "null";
      try {
        return JSON.stringify(value, bigintSafe);
      } catch {
        return String(value); // circular / non-serialisable — best effort
      }
    }
    default:
      return String(value); // number, boolean, undefined
  }
}

/** Render one `console.*` argument; strings pass through unquoted (console-style). */
export function format(arg: unknown): string {
  return typeof arg === "string" ? arg : render(arg);
}

function bigintSafe(_key: string, value: unknown): unknown {
  return typeof value === "bigint" ? value.toString() : value;
}
