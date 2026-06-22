<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount } from 'vue'
import { withBase } from 'vitepress'

/* ─────────────────────────────────────────────────────────────
   EDIT YOUR CONTENT HERE
   ───────────────────────────────────────────────────────────── */
const eyebrow = ['erlang-inspired', 'webassembly', 'rust']
const kicker = 'RUSM'
const headline = 'An Erlang-inspired WebAssembly runtime in Rust'
const tagline =
  'Isolated lightweight processes, fault tolerance, per-actor sandboxing, and secure clusters you can hook into live — on WebAssembly.'

const actions = [
  { text: 'Get started', link: '/install', primary: true },
  { text: 'Why RUSM', link: '/00-vision' },
  { text: 'Architecture', link: '/01-architecture' },
  { text: 'Roadmap', link: '/02-roadmap' },
]

const stats = [
  { value: '~2.4M', highlight: true, label: 'spawns / sec' },
  { value: '1 : 1', label: 'process : sandbox' },
  { value: 'deny', label: 'by default' },
]

// icon = an SVG path; title; html-enabled description (use <b> for cyan emphasis)
const features = [
  { icon: 'M4 7h16M4 12h10M4 17h16', title: 'Processes as Wasm instances',
    body: 'Each process is an isolated Wasm instance — own stack, heap, syscalls, and permissions. <b>One crash can never corrupt another.</b>' },
  { icon: 'M4 12h6l2-7 4 14 2-7h2', title: 'Write blocking code, get async',
    body: 'Wasmtime fibers suspend a guest&rsquo;s &ldquo;blocking&rdquo; call while the host awaits. <b>Millions can wait for almost nothing.</b>' },
  { icon: 'M5 18V8M10 18V5M15 18v-7M20 18v-4', title: 'Massive, fair concurrency',
    body: 'Tokio tasks multiplexed over a few threads, with epoch interruption for BEAM-like fairness. <b>~2.4M spawns/sec.</b>' },
  { icon: 'M12 3v6m0 0 3-3m-3 3L9 6M5 14a7 7 0 0 0 14 0', title: 'Fault tolerance',
    body: 'Traps become process exits; links and monitors propagate failure so <b>supervisors restart exactly what broke.</b>' },
  { icon: 'M7 8h10M7 12h10M7 16h6M4 4h16v16H4z', title: 'The OTP core stands alone',
    body: 'Processes, mailboxes, links, and supervisors — <b>implemented as a runtime-agnostic core.</b>' },
  { icon: 'M12 3 4 7v5c0 5 8 9 8 9s8-4 8-9V7l-8-4zM9 12l2 2 4-4', title: 'Default-deny capabilities',
    body: 'Every process gets nothing unless granted — <b>capabilities are explicit, scoped, revocable.</b>' },
]
/* ───────────────────────────────────────────────────────────── */

const COUNT = 36
const SUP = 14 // supervisor cell index
const states = ref<string[]>(
  Array.from({ length: COUNT }, (_, i) => (i === SUP ? 'sup' : 'alive')),
)
const delays = Array.from({ length: COUNT }, () => ({
  '--delay': (Math.random() * 3).toFixed(2) + 's',
  '--d': (2.4 + Math.random() * 1.6).toFixed(2) + 's',
}))

let timer: ReturnType<typeof setInterval> | undefined
const timeouts: ReturnType<typeof setTimeout>[] = []

function crash() {
  const pool: number[] = []
  for (let i = 0; i < COUNT; i++) if (i !== SUP && states.value[i] === 'alive') pool.push(i)
  if (!pool.length) return
  const i = pool[Math.floor(Math.random() * pool.length)]
  states.value[i] = 'crash'                                  // trap → exit
  timeouts.push(setTimeout(() => {                           // supervisor reacts
    states.value[SUP] = 'restart'
    timeouts.push(setTimeout(() => (states.value[SUP] = 'sup'), 420))
  }, 520))
  timeouts.push(setTimeout(() => (states.value[i] = 'restart'), 900))
  timeouts.push(setTimeout(() => (states.value[i] = 'alive'), 1320))
}

onMounted(() => {
  if (typeof window !== 'undefined' &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches) return
  timeouts.push(setTimeout(crash, 700))
  timer = setInterval(crash, 1700)
})
onBeforeUnmount(() => {
  if (timer) clearInterval(timer)
  timeouts.forEach(clearTimeout)
})
</script>

<template>
  <div class="r-home">
    <div class="r-atmos" aria-hidden="true"></div>

    <header class="r-wrap r-hero">
      <div class="r-hero-copy">
        <p class="r-eyebrow rv" style="--i: 0">
          //
          <template v-for="(w, n) in eyebrow" :key="w">
            <span :class="{ dim: n === 0 }">{{ w }}</span><span v-if="n < eyebrow.length - 1" class="sep"> · </span>
          </template>
        </p>
        <h1 class="r-h1 rv" style="--i: 1"><span class="accent">{{ kicker }}</span>{{ headline }}</h1>
        <p class="r-sub rv" style="--i: 2">{{ tagline }}</p>

        <div class="r-cta rv" style="--i: 3">
          <a v-for="a in actions" :key="a.text" :href="withBase(a.link)"
             class="r-btn" :class="{ primary: a.primary }">{{ a.text }}</a>
        </div>

        <div class="r-stats rv" style="--i: 4">
          <div class="r-stat" v-for="s in stats" :key="s.label">
            <div class="n"><b v-if="s.highlight">{{ s.value }}</b><template v-else>{{ s.value }}</template></div>
            <div class="l">{{ s.label }}</div>
          </div>
        </div>
      </div>

      <div class="r-field rv" style="--i: 3" aria-hidden="true">
        <div class="r-field-label">supervisor<span class="muted"> · </span><span class="live">●</span> {{ COUNT }} procs alive</div>
        <div class="r-scan"></div>
        <div class="r-cells">
          <div class="r-cell" v-for="(s, i) in states" :key="i" :class="s" :style="delays[i]"></div>
        </div>
      </div>
    </header>

    <section class="r-wrap r-features">
      <div class="r-feat-head"><span class="tag">// the model</span><span class="ln"></span></div>
      <div class="r-grid">
        <article class="r-card rv" v-for="(f, i) in features" :key="f.title" :style="{ '--i': i }">
          <div class="idx">{{ String(i + 1).padStart(2, '0') }}</div>
          <div class="ic">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path :d="f.icon" /></svg>
          </div>
          <h3>{{ f.title }}</h3>
          <p v-html="f.body"></p>
        </article>
      </div>
    </section>
  </div>
</template>

<style scoped>
.r-home { position: relative; }
.r-atmos {
  position: absolute; inset: 0 0 auto 0; height: 1000px; z-index: 0; pointer-events: none;
  background-image:
    linear-gradient(var(--r-line) 1px, transparent 1px),
    linear-gradient(90deg, var(--r-line) 1px, transparent 1px);
  background-size: 64px 64px;
  -webkit-mask-image: radial-gradient(120% 80% at 72% 0%, #000 26%, transparent 74%);
  mask-image: radial-gradient(120% 80% at 72% 0%, #000 26%, transparent 74%);
}
.r-atmos::after {
  content: ""; position: absolute; top: -200px; left: -160px; width: 740px; height: 740px;
  background: radial-gradient(circle, var(--r-glow), transparent 62%);
}
.r-wrap { position: relative; z-index: 1; max-width: 1180px; margin: 0 auto; padding: 0 32px; }

.r-hero { display: grid; grid-template-columns: 1.05fr .95fr; gap: 48px; align-items: center; padding: 70px 32px 60px; }
.r-eyebrow { font-family: var(--r-mono); font-size: 12.5px; letter-spacing: .04em; color: var(--r-copper); margin: 0 0 22px; }
.r-eyebrow .dim { color: var(--r-text-3); }
.r-eyebrow .sep { color: var(--r-text-3); }
.r-h1 { font-family: var(--r-display); font-weight: 800; letter-spacing: -.03em; line-height: .98;
  font-size: clamp(38px, 5.4vw, 64px); color: var(--r-text); margin: 0; }
.r-h1 .accent { display: block; font-size: .42em; letter-spacing: .01em; font-weight: 700; color: var(--r-copper); margin-bottom: .18em; }
.r-sub { font-size: 18px; color: var(--r-text-2); max-width: 30em; margin: 24px 0 32px; line-height: 1.55; }
.r-cta { display: flex; flex-wrap: wrap; gap: 12px; margin-bottom: 42px; }
.r-btn { font-weight: 600; font-size: 14.5px; padding: 12px 22px; border-radius: 999px; text-decoration: none;
  border: 1px solid var(--r-line-2); color: var(--r-text); transition: .2s; }
.r-btn:hover { border-color: var(--r-copper); color: var(--r-copper); transform: translateY(-1px); }
.r-btn.primary { background: var(--r-copper); color: var(--r-on-copper); border-color: var(--r-copper); }
.r-btn.primary:hover { background: var(--r-copper-soft); border-color: var(--r-copper-soft); color: var(--r-on-copper); }
.r-stats { display: flex; gap: 34px; border-top: 1px solid var(--r-line); padding-top: 22px; }
.r-stat .n { font-family: var(--r-display); font-weight: 700; font-size: 24px; color: var(--r-text); }
.r-stat .n b { color: var(--r-signal); font-weight: 700; }
.r-stat .l { font-family: var(--r-mono); font-size: 11px; letter-spacing: .04em; color: var(--r-text-3); margin-top: 2px; text-transform: uppercase; }

.r-field { position: relative; aspect-ratio: 1/1; width: 100%; max-width: 440px; margin-left: auto;
  background: radial-gradient(circle at 50% 45%, var(--r-glow), transparent 60%), var(--r-field);
  border: 1px solid var(--r-line); border-radius: 20px; overflow: hidden; }
.r-field-label { position: absolute; top: 14px; left: 16px; font-family: var(--r-mono); font-size: 11px; color: var(--r-text-3); letter-spacing: .04em; z-index: 3; }
.r-field-label .muted { color: var(--r-text-3); }
.r-field-label .live { color: var(--r-signal); }
.r-cells { position: absolute; inset: 40px 28px 28px; display: grid; grid-template-columns: repeat(6, 1fr); grid-template-rows: repeat(6, 1fr); gap: 11px; }
.r-cell { border-radius: 7px; background: var(--r-cell); border: 1px solid color-mix(in srgb, var(--r-signal) 32%, transparent);
  transition: transform .35s cubic-bezier(.2,.8,.2,1), background .35s, border-color .35s, box-shadow .35s; }
.r-cell.alive { animation: r-breathe var(--d, 3s) ease-in-out infinite; animation-delay: var(--delay, 0s); }
.r-cell.sup { background: color-mix(in srgb, var(--r-copper) 22%, transparent); border-color: var(--r-copper); }
.r-cell.crash { background: color-mix(in srgb, var(--r-red) 26%, transparent); border-color: var(--r-red); transform: scale(.7); }
.r-cell.restart { background: color-mix(in srgb, var(--r-copper) 40%, transparent); border-color: var(--r-copper-soft); box-shadow: 0 0 18px var(--r-glow); transform: scale(1.08); }
@keyframes r-breathe { 0%, 100% { background: var(--r-cell); } 50% { background: color-mix(in srgb, var(--r-signal) 18%, transparent); } }
.r-scan { position: absolute; left: 0; right: 0; height: 64px; z-index: 2; pointer-events: none;
  background: linear-gradient(180deg, transparent, color-mix(in srgb, var(--r-signal) 9%, transparent), transparent);
  animation: r-scan 5.5s linear infinite; }
@keyframes r-scan { 0% { top: -64px; } 100% { top: 100%; } }

.r-features { padding-bottom: 90px; }
.r-feat-head { display: flex; align-items: baseline; gap: 14px; margin: 24px 0 26px; }
.r-feat-head .tag { font-family: var(--r-mono); font-size: 12px; color: var(--r-copper); letter-spacing: .04em; }
.r-feat-head .ln { flex: 1; height: 1px; background: var(--r-line); }
.r-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 18px; }
.r-card { background: var(--r-surface); border: 1px solid var(--r-line); border-radius: 14px; padding: 26px 24px 28px; position: relative; overflow: hidden; transition: .25s; }
.r-card::before { content: ""; position: absolute; top: 0; left: 0; right: 0; height: 2px; background: var(--r-copper); transform: scaleX(0); transform-origin: left; transition: transform .35s; }
.r-card:hover { transform: translateY(-4px); border-color: var(--r-line-2); }
.r-card:hover::before { transform: scaleX(1); }
.r-card .idx { font-family: var(--r-mono); font-size: 12px; color: var(--r-text-3); }
.r-card .ic { width: 34px; height: 34px; border-radius: 9px; display: flex; align-items: center; justify-content: center; margin: 14px 0 16px; color: var(--r-copper); background: color-mix(in srgb, var(--r-copper) 14%, transparent); }
.r-card h3 { font-family: var(--r-display); font-weight: 700; font-size: 19px; letter-spacing: -.01em; margin: 0 0 10px; color: var(--r-text); }
.r-card :deep(p) { font-size: 14.5px; color: var(--r-text-2); line-height: 1.62; margin: 0; }
.r-card :deep(p b) { color: var(--r-signal); font-weight: 600; }

.rv { opacity: 0; transform: translateY(16px); animation: r-rise .7s cubic-bezier(.2,.8,.2,1) forwards; animation-delay: calc(var(--i, 0) * .06s + .05s); }
@keyframes r-rise { to { opacity: 1; transform: none; } }
@media (prefers-reduced-motion: reduce) {
  .rv, .r-cell.alive, .r-scan { animation: none; opacity: 1; transform: none; }
}
@media (max-width: 860px) {
  .r-hero { grid-template-columns: 1fr; gap: 36px; padding-top: 48px; }
  .r-field { max-width: 380px; margin: 0 auto; }
  .r-grid { grid-template-columns: 1fr; }
}
</style>
