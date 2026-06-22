import { defineConfig } from 'vitepress';

// One source of truth for navigation: the same grouped structure drives both the
// top nav (as dropdowns) and the sidebar (as sections), so they never diverge.
const sections = [
  {
    // Orient + get running.
    text: 'Introduction',
    items: [
      {
        text: 'Overview',
        items: [
          { text: 'Why RUSM?', link: '/introduction/why-rusm' },
          { text: 'What you get', link: '/introduction/what-you-get' },
        ],
      },
      {
        text: 'Getting started',
        items: [
          { text: 'Install', link: '/introduction/install' },
          { text: 'Quick start', link: '/introduction/quick-start' },
        ],
      },
    ],
  },
  {
    // The hands-on how-to, in the order you build: set up, write a component, serve the
    // web, then common patterns, then securing and extending the app.
    text: 'Build an app',
    items: [
      {
        text: 'Set up',
        items: [
          { text: 'The app model', link: '/build-an-app/app-model' },
          { text: 'The rusm CLI', link: '/build-an-app/cli' },
        ],
      },
      {
        text: 'Write a component',
        items: [
          { text: 'TypeScript', link: '/build-an-app/typescript-component' },
          { text: 'Rust', link: '/build-an-app/rust-component' },
          { text: 'Go', link: '/build-an-app/go-component' },
        ],
      },
      {
        text: 'Serve the web',
        items: [
          { text: 'Serve HTTP', link: '/build-an-app/serve-http' },
          { text: 'Serve WebSocket', link: '/build-an-app/serve-websocket' },
          { text: 'Serve SSE', link: '/build-an-app/serve-sse' },
        ],
      },
      {
        text: 'Common patterns',
        items: [
          { text: 'Call another component', link: '/build-an-app/call-another-component' },
          { text: 'Run one-off work', link: '/build-an-app/run-one-off-work' },
          { text: 'Build a stateful service', link: '/build-an-app/stateful-service' },
          { text: 'Broadcast to many', link: '/build-an-app/broadcast' },
          { text: 'Coordinate & supervise', link: '/build-an-app/coordinate-and-supervise' },
        ],
      },
      {
        text: 'Secure',
        items: [{ text: 'Grant capabilities', link: '/build-an-app/capabilities' }],
      },
      {
        text: 'Extend',
        items: [{ text: 'Add your own functions', link: '/build-an-app/custom-bridges' }],
      },
    ],
  },
  {
    // Advanced topics + how it works underneath.
    text: 'Deep dive',
    items: [
      {
        text: 'The actor model',
        items: [
          { text: 'The process model', link: '/deep-dive/wasm-instance-as-process' },
          { text: 'Message passing', link: '/deep-dive/message-passing' },
          { text: 'Links & supervision', link: '/deep-dive/links-and-supervision' },
          { text: 'Fibers & blocking→async', link: '/deep-dive/fibers-and-blocking-to-async' },
          { text: 'Epoch preemption', link: '/deep-dive/epoch-preemption' },
          { text: 'Process management', link: '/deep-dive/process-management' },
        ],
      },
      {
        text: 'Components & guests',
        items: [
          { text: 'Components & the actor world', link: '/deep-dive/components-and-the-actor-world' },
          { text: 'Component lifecycles', link: '/deep-dive/component-lifecycle' },
          { text: 'Guests: Rust, TypeScript & Go', link: '/deep-dive/guests' },
          { text: 'Permissions & sandboxing', link: '/deep-dive/permissions-and-sandboxing' },
        ],
      },
      {
        text: 'Serving & streaming',
        items: [
          { text: 'The serving model', link: '/deep-dive/serving-model' },
          { text: 'Serving HTTP, WS & SSE', link: '/deep-dive/serving-http-ws-sse' },
          { text: 'Byte streams', link: '/deep-dive/byte-streams' },
        ],
      },
      {
        text: 'Apps, clusters & ops',
        items: [
          { text: 'The app model', link: '/deep-dive/app-model' },
          { text: 'Distributed nodes', link: '/deep-dive/distributed-nodes' },
          { text: 'The distributed model', link: '/deep-dive/distributed-model' },
          { text: 'Live attach', link: '/deep-dive/live-attach' },
          { text: 'Observe a node', link: '/deep-dive/observe' },
          { text: 'Embedding RUSM as a library', link: '/deep-dive/embedding' },
        ],
      },
      {
        // The exhaustive lookup specs (the teaching is in the Guide; this is the spec).
        text: 'Reference',
        items: [
          { text: 'Configuration', link: '/deep-dive/configuration' },
          { text: 'Host ABI', link: '/deep-dive/host-abi' },
          { text: 'Glossary', link: '/deep-dive/glossary' },
        ],
      },
    ],
  },
  {
    // Background: how RUSM compares, and the project itself.
    text: 'About',
    items: [
      {
        text: 'Comparisons',
        items: [
          { text: 'RUSM vs Lunatic', link: '/about/rusm-vs-lunatic' },
          { text: 'How RUSM compares', link: '/about/comparison' },
          { text: 'Design analysis', link: '/about/design-analysis' },
        ],
      },
      {
        text: 'The project',
        items: [
          { text: 'Architecture', link: '/about/architecture' },
          { text: 'Roadmap', link: '/about/roadmap' },
          { text: 'Development', link: '/about/development' },
          { text: 'Benchmark & dashboard', link: '/about/benchmark-dashboard' },
        ],
      },
    ],
  },
  {
    // Phases grouped by the arc the roadmap tells (foundation → OTP core → Wasm →
    // distributed & scale); short `PN —` labels.
    text: 'Phase log',
    items: [
      {
        text: 'Foundation',
        items: [{ text: 'P0 — Foundation', link: '/phases/phase-00-foundation' }],
      },
      {
        text: 'OTP core',
        items: [
          { text: 'P1 — Process core', link: '/phases/phase-01-process-core' },
          { text: 'P2 — Messaging', link: '/phases/phase-02-messaging' },
          { text: 'P3 — Supervision', link: '/phases/phase-03-supervision' },
          { text: 'P4 — Management', link: '/phases/phase-04-management' },
          { text: 'P5 — TCP', link: '/phases/phase-05-tcp' },
        ],
      },
      {
        text: 'WebAssembly',
        items: [
          { text: 'P6 — Wasm backend', link: '/phases/phase-06-wasm-backend' },
          { text: 'P7 — Component hosting', link: '/phases/phase-07-components' },
          { text: 'P8 — Guest ergonomics', link: '/phases/phase-08-guest-ergonomics' },
        ],
      },
      {
        text: 'Distributed & scale',
        items: [
          { text: 'P9 — Distributed clusters', link: '/phases/phase-09-distributed-clusters' },
          { text: 'P10 — Scale & hardening', link: '/phases/phase-10-scale-hardening' },
        ],
      },
    ],
  },
];

export default defineConfig({
  title: 'RUSM',
  description: 'An Erlang-inspired WebAssembly runtime in Rust.',
  // Served as a GitHub Pages project site at https://archan937.github.io/rusm/,
  // so every asset/link resolves under the /rusm/ subpath.
  base: '/rusm/',
  cleanUrls: true,
  // Four code-theme families, each a light/dark pair, all baked into every code block
  // as `--shiki-<key>` CSS variables (Shiki multi-theme via `defaultColor:false`). The
  // `light`/`dark` keys are the default (Rosé Pine — warm, matches the copper/cream
  // brand); a nav-bar switcher (theme/CodeThemeToggle.vue) flips `data-code-theme` on
  // <html> to remap which pair is live. Extra themes are registered in `shikiSetup`
  // (VitePress only auto-loads `light`+`dark`).
  markdown: {
    theme: {
      light: 'rose-pine-dawn',
      dark: 'rose-pine-moon',
      catpLight: 'catppuccin-latte',
      catpDark: 'catppuccin-mocha',
      vitLight: 'vitesse-light',
      vitDark: 'vitesse-dark',
      oneLight: 'one-light',
      oneDark: 'one-dark-pro',
      // VitePress's type only models { light, dark }; Shiki accepts the full record at
      // runtime (each key → a `--shiki-<key>` var), so cast past the narrow type.
    } as any,
    async shikiSetup(highlighter) {
      await highlighter.loadTheme(
        'catppuccin-latte',
        'catppuccin-mocha',
        'vitesse-light',
        'vitesse-dark',
        'one-light',
        'one-dark-pro',
      );
    },
  },
  // The RUSM theme's fonts (display / base / mono), loaded with preconnect for
  // performance rather than a CSS @import.
  head: [
    // Restore the saved code-theme before first paint so reloads don't flash the default.
    [
      'script',
      {},
      "try{var t=localStorage.getItem('rusm-code-theme');if(t)document.documentElement.dataset.codeTheme=t;}catch(e){}",
    ],
    ['link', { rel: 'preconnect', href: 'https://fonts.googleapis.com' }],
    ['link', { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: '' }],
    [
      'link',
      {
        rel: 'stylesheet',
        href: 'https://fonts.googleapis.com/css2?family=Bricolage+Grotesque:opsz,wght@12..96,500;12..96,700;12..96,800&family=Hanken+Grotesk:wght@400;500;600&family=JetBrains+Mono:wght@400;500&display=swap',
      },
    ],
  ],
  themeConfig: {
    nav: sections,
    sidebar: sections,
    search: { provider: 'local' },
    socialLinks: [{ icon: 'github', link: 'https://github.com/archan937/rusm' }],
    footer: {
      message: 'MIT licensed',
      copyright: '© Paul Engel',
    },
  },
});
