// The REPL evaluation engine: a stateful JavaScript scope that evaluates one line
// at a time, persisting bindings across lines and capturing console output. Pure
// (no actor/host dependency) so it unit-tests with a plain `new Session()`.

import { format, render } from "./render";
import { transform } from "./transform";

export interface EvalResult {
  /** The rendered return value; empty when the line yields nothing. */
  value: string;
  /** Captured `console.*` lines, in emission order. */
  output: string[];
  /** The error message when the line threw or failed to parse; otherwise null. */
  error: string | null;
}

const CONSOLE_LEVELS = ["log", "info", "warn", "error", "debug"] as const;

/** Redirect `console.*` into a buffer for the duration of one eval. */
function captureConsole(): { lines: () => string[]; restore: () => void } {
  const lines: string[] = [];
  const saved = new Map<string, unknown>();
  for (const level of CONSOLE_LEVELS) {
    saved.set(level, (console as Record<string, unknown>)[level]);
    (console as Record<string, unknown>)[level] = (...args: unknown[]) =>
      lines.push(args.map(format).join(" "));
  }
  return {
    lines: () => [...lines],
    restore: () => {
      for (const level of CONSOLE_LEVELS) {
        (console as Record<string, unknown>)[level] = saved.get(level);
      }
    },
  };
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? `${e.name}: ${e.message}` : String(e);
}

/** A persistent REPL scope. One per attach connection. */
export class Session {
  private scope: Record<string, unknown> = {};
  private known: string[] = [];

  /**
   * Evaluate one line. Never throws: a parse error, runtime throw, or rejected
   * promise becomes `{ error }`, and the session stays usable for the next line.
   * Bindings only advance on success, so a failed line leaves the scope untouched.
   */
  async eval(code: string): Promise<EvalResult> {
    const t = transform(code, this.known);
    if (t.error) {
      return { value: "", output: [], error: t.error };
    }
    const console = captureConsole();
    try {
      // eslint-disable-next-line no-new-func — a REPL is, by definition, eval.
      const run = new Function("__repl_S", t.body) as (s: Record<string, unknown>) => Promise<unknown>;
      const result = await run(this.scope);
      this.known = t.names;
      return {
        value: result === undefined ? "" : render(result),
        output: console.lines(),
        error: null,
      };
    } catch (e) {
      return { value: "", output: console.lines(), error: errorMessage(e) };
    } finally {
      console.restore();
    }
  }
}
