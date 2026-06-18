// The web UI served at GET / by the `api`. It is deliberately a single, dependency-free
// HTML page (no build step, no framework) so the focus stays on what RUSM is doing — every
// section says which component and capability it exercises. The board talks to the api
// (same origin); the chat talks to the WS listener; the feed is explained with a one-liner
// to watch the live push.
export const page = /* html */ `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>RUSM todo board</title>
  <style>
    :root { font: 15px/1.5 system-ui, sans-serif; color: #1a1a1a; }
    body { max-width: 52rem; margin: 2rem auto; padding: 0 1rem; }
    h1 { margin-bottom: .2rem; }
    .sub { color: #666; margin-top: 0; }
    section { border: 1px solid #e3e3e3; border-radius: 8px; padding: 1rem 1.2rem; margin: 1rem 0; }
    h2 { margin: 0 0 .2rem; font-size: 1.05rem; }
    .what { color: #555; font-size: .9rem; margin: 0 0 .8rem; }
    .what code { background: #f3f3f3; padding: .05rem .3rem; border-radius: 4px; }
    ul { list-style: none; padding: 0; margin: .5rem 0 0; }
    li { display: flex; align-items: center; gap: .5rem; padding: .3rem 0; border-top: 1px solid #f0f0f0; }
    li.done span { text-decoration: line-through; color: #999; }
    li span { flex: 1; }
    button { font: inherit; cursor: pointer; border: 1px solid #ccc; background: #fafafa; border-radius: 6px; padding: .2rem .6rem; }
    input { font: inherit; padding: .35rem .5rem; border: 1px solid #ccc; border-radius: 6px; }
    .row { display: flex; gap: .5rem; margin-top: .6rem; }
    .row input { flex: 1; }
    #chatlog { background: #fafafa; border: 1px solid #eee; border-radius: 6px; padding: .5rem; height: 9rem; overflow: auto; font-size: .85rem; white-space: pre-wrap; }
  </style>
</head>
<body>
  <h1>RUSM todo board</h1>
  <p class="sub">One app, three serving shapes + a service — each an isolated, supervised WASM process, unified by process-group tags.</p>

  <section>
    <h2>Todos — HTTP <span class="what">(the <code>api</code> component, :8080)</span></h2>
    <p class="what">A web-standard <code>fetch</code> handler doing its own routing. CRUD persists in durable <code>kv</code>; every change is published to the <code>feed</code>'s subscribers over a tag. (A resident <code>store</code> service exposes the same data to the typed-client demo in <code>reporter</code>.)</p>
    <div class="row">
      <input id="text" placeholder="What needs doing?" />
      <button id="add">Add</button>
    </div>
    <ul id="todos"></ul>
  </section>

  <section>
    <h2>Live feed — SSE <span class="what">(the <code>feed</code> component, :8081)</span></h2>
    <p class="what">A process per connection that subscribes to the todo tag and streams each change — true push, not polling. The list above refreshes as todos change anywhere; to watch the raw stream:</p>
    <pre class="what"><code>curl -N localhost:8081</code></pre>
  </section>

  <section>
    <h2>Chat — WebSocket <span class="what">(the <code>chat</code> component, :8082)</span></h2>
    <p class="what">A process per connection; rooms are tags, fan-out is <code>whereisTag</code> + <code>send</code>. Join a room, then send.</p>
    <div class="row">
      <input id="room" placeholder="room (e.g. general)" />
      <button id="join">Join</button>
    </div>
    <div class="row">
      <input id="say" placeholder="message" />
      <button id="send">Send</button>
    </div>
    <div id="chatlog"></div>
  </section>

  <script>
    const $ = (id) => document.getElementById(id);

    // --- Todos (api, same origin) ---
    async function load() {
      const todos = await fetch("/todos").then((r) => r.json());
      $("todos").innerHTML = "";
      for (const t of todos) {
        const li = document.createElement("li");
        if (t.done) li.className = "done";
        li.innerHTML = '<span></span><button class="t">toggle</button><button class="d">delete</button>';
        li.querySelector("span").textContent = "#" + t.id + " " + t.text;
        li.querySelector(".t").onclick = () => fetch("/todos/" + t.id, { method: "PATCH" }).then(load);
        li.querySelector(".d").onclick = () => fetch("/todos/" + t.id, { method: "DELETE" }).then(load);
        $("todos").appendChild(li);
      }
    }
    $("add").onclick = async () => {
      const text = $("text").value.trim();
      if (!text) return;
      await fetch("/todos", { method: "POST", body: JSON.stringify({ text }) });
      $("text").value = "";
      load();
    };
    load();
    setInterval(load, 1500); // reflect changes from other clients (the feed pushes the same data live)

    // --- Chat (WebSocket) ---
    const log = (line) => { $("chatlog").textContent += line + "\\n"; $("chatlog").scrollTop = 1e9; };
    const ws = new WebSocket("ws://" + location.hostname + ":8082");
    ws.onmessage = (e) => log(e.data);
    ws.onopen = () => log("(connected)");
    ws.onclose = () => log("(disconnected)");
    $("join").onclick = () => ws.send(JSON.stringify({ join: $("room").value.trim() || "general" }));
    $("send").onclick = () => { ws.send(JSON.stringify({ say: $("say").value })); $("say").value = ""; };
  </script>
</body>
</html>`;
