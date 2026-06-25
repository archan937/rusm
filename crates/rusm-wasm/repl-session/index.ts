// The REPL session component: a long-lived, sandboxed process holding one
// JavaScript scope, evaluating lines the node forwards over its mailbox.
//
// Wire (JSON text, both directions):
//   host → session:  { "code": "<js>", "replyTo": "<pid>" }
//   session → host:  { "value": "<rendered>", "output": ["…"], "error": null | "…" }
//
// One process per attach connection; bindings are private to it and die with it.
// This is platform code (it ships with the node), written on the guest `Process`
// API like any other component.

import { Session } from "./engine";

declare const Process: {
  receiveText(): Promise<string>;
  send(to: bigint | string, message: string): void;
};

interface EvalRequest {
  code: string;
  replyTo: string;
}

export default async function (): Promise<void> {
  const session = new Session();
  for (;;) {
    const request = JSON.parse(await Process.receiveText()) as EvalRequest;
    const result = await session.eval(request.code);
    Process.send(request.replyTo, JSON.stringify(result));
  }
}
