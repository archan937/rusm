import { defineConfig } from 'vitepress';

// One source of truth for navigation: the same grouped structure drives both the
// top nav (as dropdowns) and the sidebar (as sections), so they never diverge.
const sections = [
  {
    // Categorised like every other section — and the sub-headings of the (long)
    // getting-started page are surfaced as anchor links so the whole arc is
    // navigable from the sidebar, not just the first three headings.
    text: 'Get started',
    items: [
      {
        text: 'Setup',
        items: [
          { text: 'Overview', link: '/getting-started#overview' },
          { text: 'Install', link: '/getting-started#install' },
          { text: 'Quick start', link: '/getting-started#quick-start' },
        ],
      },
      {
        text: 'Ways to use RUSM',
        items: [
          { text: 'The OTP core (no Wasm)', link: '/getting-started#otp-core' },
          { text: 'Embed a .wasm', link: '/getting-started#embed-wasm' },
          { text: 'The app model', link: '/getting-started#app-model' },
        ],
      },
      {
        text: 'Write a component',
        items: [
          { text: 'Rust component', link: '/getting-started#rust-component' },
          { text: 'TypeScript component', link: '/getting-started#ts-component' },
          { text: 'Serve over HTTP/WS/SSE', link: '/getting-started#serve' },
        ],
      },
      {
        text: 'Inside a component',
        items: [
          { text: 'Process management', link: '/getting-started#process-management' },
          { text: 'Streaming', link: '/getting-started#streaming' },
          { text: 'Capabilities & sandboxing', link: '/getting-started#capabilities' },
          { text: 'Observe a node', link: '/getting-started#observe' },
        ],
      },
    ],
  },
  {
    text: 'About RUSM',
    items: [
      {
        text: 'Overview',
        items: [
          { text: 'Why RUSM?', link: '/00-vision' },
          { text: 'Features', link: '/features' },
        ],
      },
      {
        text: 'Comparisons',
        items: [
          { text: 'RUSM vs Lunatic', link: '/lunatic-comparison' },
          { text: 'How RUSM compares', link: '/comparison' },
          { text: 'Design analysis', link: '/design-analysis' },
        ],
      },
      {
        text: 'The project',
        items: [
          { text: 'Architecture', link: '/01-architecture' },
          { text: 'Roadmap', link: '/02-roadmap' },
          { text: 'Development', link: '/06-development' },
        ],
      },
    ],
  },
  {
    // Grouped into categories (see /features for the value-first map). One level of
    // nesting renders as grouped sections in both the sidebar and the nav dropdown.
    text: 'Concepts',
    items: [
      {
        text: 'The actor model',
        items: [
          { text: 'The process model', link: '/concepts/wasm-instance-as-process' },
          { text: 'Message passing', link: '/concepts/message-passing' },
          { text: 'Links & supervision', link: '/concepts/links-and-supervision' },
          { text: 'Fibers & blocking→async', link: '/concepts/fibers-and-blocking-to-async' },
          { text: 'Epoch preemption', link: '/concepts/epoch-preemption' },
          { text: 'Process management', link: '/concepts/process-management' },
        ],
      },
      {
        text: 'Component lifecycles',
        items: [
          { text: 'Overview', link: '/concepts/component-lifecycle' },
          { text: 'HTTP component', link: '/concepts/lifecycle-http' },
          { text: 'SSE component', link: '/concepts/lifecycle-sse' },
          { text: 'WebSocket component', link: '/concepts/lifecycle-websocket' },
          { text: 'Worker component (per-call)', link: '/concepts/lifecycle-worker' },
          { text: 'Service component (resident)', link: '/concepts/lifecycle-service' },
        ],
      },
      {
        text: 'WebAssembly & safety',
        items: [
          { text: 'Components & the actor world', link: '/concepts/components-and-the-actor-world' },
          { text: 'Permissions & sandboxing', link: '/concepts/permissions-and-sandboxing' },
          { text: 'Guests: Rust, TypeScript & Go', link: '/concepts/guests' },
        ],
      },
      {
        text: 'Serving & streaming',
        items: [
          { text: 'The serving model', link: '/concepts/serving-model' },
          { text: 'Byte streams', link: '/concepts/byte-streams' },
        ],
      },
      {
        text: 'Apps & clusters',
        items: [
          { text: 'The app model', link: '/concepts/app-model' },
          { text: 'Distributed nodes', link: '/concepts/distributed-nodes' },
          { text: 'Live attach', link: '/concepts/live-attach' },
        ],
      },
    ],
  },
  {
    text: 'Reference',
    items: [
      {
        text: 'CLI & configuration',
        items: [
          { text: 'The rusm CLI', link: '/reference-cli' },
          { text: 'Configuration', link: '/reference-configuration' },
        ],
      },
      {
        text: 'APIs & models',
        items: [
          { text: 'Host ABI', link: '/05-host-abi' },
          { text: 'Distributed model', link: '/04-distributed-model' },
          { text: 'Serving HTTP/WS/SSE', link: '/serving-http-ws-sse' },
        ],
      },
      {
        text: 'Tools & appendix',
        items: [
          { text: 'Benchmark & dashboard', link: '/03-benchmark-dashboard' },
          { text: 'Glossary', link: '/07-glossary' },
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
