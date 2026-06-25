import { describe, expect, test } from "bun:test";

import { Session } from "./engine";
import { render } from "./render";
import { transform } from "./transform";

describe("transform", () => {
  test("blanks a const keyword and tracks the binding", () => {
    const t = transform("const p = 41", []);
    expect(t.error).toBeUndefined();
    expect(t.names).toEqual(["p"]);
    expect(t.body).toContain("p = 41"); // keyword blanked → assignment
    expect(t.body).toContain("Object.assign(__repl_S, { p })");
    expect(t.body).not.toContain("({  } = __repl_S)"); // nothing to import yet
  });

  test("imports previously-known bindings", () => {
    const t = transform("p + 1", ["p"]);
    expect(t.body).toContain("({ p } = __repl_S)");
    expect(t.body).toContain("__repl_ret = (p + 1)");
  });

  test("rewrites function and class declarations to assignments", () => {
    expect(transform("function f(){}", []).body).toContain("f = function f(){}");
    expect(transform("class C {}", []).body).toContain("C = class C {}");
  });

  test("tracks a bare assignment as a binding", () => {
    expect(transform("pid = 7", []).names).toEqual(["pid"]);
  });

  test("reports a parse error and keeps the prior names", () => {
    const t = transform("const = ;", ["a"]);
    expect(t.error).toContain("SyntaxError");
    expect(t.names).toEqual(["a"]);
  });
});

describe("render", () => {
  test("renders each value kind for a REPL", () => {
    expect(render(42)).toBe("42");
    expect(render(123n)).toBe("123"); // pid
    expect(render("hi")).toBe('"hi"');
    expect(render(null)).toBe("null");
    expect(render(true)).toBe("true");
    expect(render([1, 2])).toBe("[1,2]");
    expect(render({ a: 1 })).toBe('{"a":1}');
    expect(render([7n])).toBe('["7"]'); // bigint inside an object survives
    expect(render(function foo() {})).toBe("[Function: foo]");
  });
});

describe("Session — stateful evaluation", () => {
  test("evaluates an expression", async () => {
    expect(await new Session().eval("1 + 1")).toEqual({ value: "2", output: [], error: null });
  });

  test("a declaration yields no value but persists", async () => {
    const s = new Session();
    expect((await s.eval("const p = 41")).value).toBe("");
    expect((await s.eval("p + 1")).value).toBe("42");
  });

  test("a bare assignment persists (the easy case)", async () => {
    const s = new Session();
    expect((await s.eval("pid = 41")).value).toBe("41");
    expect((await s.eval("pid")).value).toBe("41");
  });

  test("supports top-level await", async () => {
    expect((await new Session().eval("await Promise.resolve(7)")).value).toBe("7");
  });

  test("persists a binding declared with top-level await (no gap)", async () => {
    const s = new Session();
    await s.eval("const x = await Promise.resolve(5)");
    expect((await s.eval("x")).value).toBe("5");
  });

  test("functions and classes persist", async () => {
    const s = new Session();
    await s.eval("function add(a, b) { return a + b }");
    expect((await s.eval("add(2, 3)")).value).toBe("5");
    await s.eval("class C { v() { return 9 } }");
    expect((await s.eval("new C().v()")).value).toBe("9");
  });

  test("captures console output in order", async () => {
    const r = await new Session().eval("console.log('hi', 42); console.warn('!'); 1");
    expect(r.output).toEqual(["hi 42", "!"]);
    expect(r.value).toBe("1");
  });

  test("a throw is reported and the session survives", async () => {
    const s = new Session();
    expect((await s.eval("throw new Error('boom')")).error).toBe("Error: boom");
    expect((await s.eval("1 + 1")).value).toBe("2");
  });

  test("a syntax error is reported and the session survives", async () => {
    const s = new Session();
    expect((await s.eval("const = ;")).error).toContain("SyntaxError");
    expect((await s.eval("2 + 2")).value).toBe("4");
  });

  test("re-declaring across lines just rebinds (REPL leniency)", async () => {
    const s = new Session();
    await s.eval("const p = 1");
    await s.eval("const p = 2");
    expect((await s.eval("p")).value).toBe("2");
  });

  test("a failed line does not advance the scope", async () => {
    const s = new Session();
    await s.eval("const a = 1");
    await s.eval("b"); // ReferenceError — b was never bound
    expect((await s.eval("a")).value).toBe("1");
  });
});
