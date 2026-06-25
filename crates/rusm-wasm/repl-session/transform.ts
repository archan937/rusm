// Rewrites one REPL line into the body of `new Function("__repl_S", <body>)` so
// that bindings persist across lines and top-level `await` works — the same
// problem Node's REPL solves, and (like Node) we solve it with a real parser
// rather than fragile regexes.
//
// The shape we emit:
//
//     return (async () => {
//       let p, pid, __repl_ret;          // every known + newly-declared name
//       ({ p, pid } = __repl_S);         // import the previously-bound values
//       p = 41;                          // user line, declarations → assignments
//       __repl_ret = (p + 1);            // the last expression's value
//       Object.assign(__repl_S, { p, pid });  // export back for the next line
//       return __repl_ret;
//     })();
//
// Persistence lives entirely in the explicit `__repl_S` scope object — never in
// `globalThis` or `with` — so the engine is isolated and unit-testable with a
// fresh scope per call. Declarations are rewritten to plain assignments against
// pre-declared `let`s, which is what makes `const`/`let`/`class`/`function` and
// bare assignments all survive to the next line, strict-mode included.

import { parse } from "acorn";

export interface Transformed {
  /** Body for `new Function("__repl_S", body)`; runs the line, returns its value. */
  body: string;
  /** All binding names known after this line — feed back in on the next call. */
  names: string[];
  /** Set when the line failed to parse; `body`/`names` are then the prior state. */
  error?: string;
}

interface Edit {
  start: number;
  end: number;
  text: string;
}

/** Identifier names bound at the top level of one parsed line. */
function topLevelBindings(body: any[], edits: Edit[]): string[] {
  const names: string[] = [];
  for (const stmt of body) {
    if (
      stmt.type === "VariableDeclaration" &&
      stmt.declarations.every((d: any) => d.id.type === "Identifier")
    ) {
      // Blank the `const`/`let`/`var` keyword → plain assignment(s) to the scope.
      // (Destructuring declarations are left intact: they run for the line but
      // don't persist — rare in a REPL, and rewriting them safely is out of scope.)
      edits.push({ start: stmt.start, end: stmt.start + stmt.kind.length, text: "" });
      for (const d of stmt.declarations) names.push(d.id.name);
    } else if (stmt.type === "FunctionDeclaration" && stmt.id) {
      // `function f(){}` → `f = function f(){}` so it persists as a binding.
      edits.push({ start: stmt.start, end: stmt.start, text: `${stmt.id.name} = ` });
      names.push(stmt.id.name);
    } else if (stmt.type === "ClassDeclaration" && stmt.id) {
      // `class C {}` → `C = class C {}` (a class declaration never persists otherwise).
      edits.push({ start: stmt.start, end: stmt.start, text: `${stmt.id.name} = ` });
      names.push(stmt.id.name);
    } else if (
      stmt.type === "ExpressionStatement" &&
      stmt.expression.type === "AssignmentExpression" &&
      stmt.expression.operator === "=" &&
      stmt.expression.left.type === "Identifier"
    ) {
      // A bare `pid = …` (no keyword): track it so the binding persists too.
      names.push(stmt.expression.left.name);
    }
  }
  return names;
}

function applyEdits(src: string, edits: Edit[]): string {
  // Apply right-to-left so earlier offsets stay valid; edits never overlap (each
  // targets a distinct statement, or a keyword within one).
  let out = src;
  for (const e of [...edits].sort((a, b) => b.start - a.start)) {
    out = out.slice(0, e.start) + e.text + out.slice(e.end);
  }
  return out;
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? `${e.name}: ${e.message}` : String(e);
}

export function transform(code: string, known: string[]): Transformed {
  let program: any;
  try {
    program = parse(code, { ecmaVersion: "latest", allowAwaitOutsideFunction: true });
  } catch (e) {
    return { body: "", names: known, error: errorMessage(e) };
  }

  const edits: Edit[] = [];
  const body = program.body as any[];
  const lineNames = topLevelBindings(body, edits);

  // The line's value is the last top-level expression, captured into __repl_ret.
  const last = body[body.length - 1];
  if (last && last.type === "ExpressionStatement") {
    const expr = code.slice(last.expression.start, last.expression.end);
    edits.push({ start: last.start, end: last.end, text: `__repl_ret = (${expr});` });
  }

  const rewritten = applyEdits(code, edits);
  const names = [...new Set([...known, ...lineNames])];

  const declared = names.length ? `let ${names.join(", ")}, __repl_ret;` : `let __repl_ret;`;
  const imported = known.length ? `({ ${known.join(", ")} } = __repl_S);` : "";
  const exported = names.length ? `Object.assign(__repl_S, { ${names.join(", ")} });` : "";

  const inner = [declared, imported, rewritten, exported, "return __repl_ret;"]
    .filter((line) => line.length)
    .join("\n");
  return { body: `return (async () => {\n${inner}\n})();`, names };
}
