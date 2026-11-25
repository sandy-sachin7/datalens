---
title: "DeltaLens Benchmark Poster"
theme: white
font: "Noto Sans"
color_background: "#F5F7F9"
color_foreground: "#111820"
color_dark_foreground: "#4A5868"
color_accent: "#164E9E"
---

```svg
<svg viewBox="0 0 1600 900" xmlns="http://www.w3.org/2000/svg">
  <!-- Background plate -->
  <rect x="0" y="0" width="1600" height="900" fill="var(--background)"/>

  <!-- Architectural perimeter rules -->
  <line x1="88" y1="72" x2="1512" y2="72" stroke="var(--foreground)" stroke-width="2.5"/>
  <line x1="88" y1="828" x2="1512" y2="828" stroke="#CBD5E0" stroke-width="1.5"/>
  <line x1="652" y1="108" x2="652" y2="796" stroke="#CBD5E0" stroke-width="1.5"/>

  <!-- Left column: identity and headline metrics -->
  <text x="88" y="132" font-family="JetBrainsMono Nerd Font" font-size="18" fill="var(--dark-foreground)">Rust CLI and library for _delta_log/</text>

  <text x="88" y="236" font-family="Noto Sans" font-size="84" font-weight="700" fill="var(--foreground)">deltalens</text>

  <text x="88" y="294" font-family="Noto Sans" font-size="25" fill="var(--dark-foreground)">Inspect, diff, and CI-gate Delta Lake tables</text>
  <text x="88" y="330" font-family="Noto Sans" font-size="25" fill="var(--dark-foreground)">directly from disk or S3. No Spark or JVM.</text>

  <rect x="88" y="376" width="76" height="4" fill="var(--accent)"/>

  <!-- Monumental metric 1 -->
  <text x="88" y="482" font-family="JetBrainsMono Nerd Font" font-size="68" font-weight="700" fill="var(--accent)">111 ms</text>
  <text x="88" y="518" font-family="Noto Sans" font-size="20" fill="var(--dark-foreground)">Full replay of 1,000 commits and 50,000 files</text>

  <line x1="88" y1="562" x2="592" y2="562" stroke="#CBD5E0" stroke-width="1.5"/>

  <!-- Monumental metric 2 -->
  <text x="88" y="660" font-family="JetBrainsMono Nerd Font" font-size="68" font-weight="700" fill="var(--foreground)">89 MB</text>
  <text x="88" y="696" font-family="Noto Sans" font-size="20" fill="var(--dark-foreground)">Peak RSS (22.9x less RAM than delta-rs Python)</text>

  <!-- Colophon -->
  <text x="88" y="788" font-family="JetBrainsMono Nerd Font" font-size="16" fill="var(--dark-foreground)">v2.0.1  |  13.8 MB binary  |  MIT</text>

  <!-- Right column: Panel A - Latency comparison (bar width 648px = 400 ms => 1.62 px/ms) -->
  <text x="708" y="132" font-family="Noto Sans" font-size="23" font-weight="700" fill="var(--foreground)">Full replay latency (50,000 files, no checkpoint)</text>
  <text x="708" y="162" font-family="JetBrainsMono Nerd Font" font-size="15" fill="var(--dark-foreground)">Lower is faster. Measured on AMD Ryzen 5 5600H, Linux x86_64.</text>

  <!-- Bar 1: deltalens 111.01 ms -> 180 px -->
  <text x="708" y="214" font-family="Noto Sans" font-size="18" font-weight="700" fill="var(--foreground)">deltalens 2.0.1 (cold CLI)</text>
  <rect x="708" y="226" width="648" height="36" fill="#E6ECF2"/>
  <rect x="708" y="226" width="180" height="36" fill="var(--accent)"/>
  <text x="1512" y="252" text-anchor="end" font-family="JetBrainsMono Nerd Font" font-size="21" font-weight="700" fill="var(--accent)">111.01 ms</text>

  <!-- Bar 2: deltalake warm 285.46 ms -> 462 px -->
  <text x="708" y="294" font-family="Noto Sans" font-size="18" fill="var(--dark-foreground)">deltalake 1.7.0 (warm in-process)</text>
  <rect x="708" y="306" width="648" height="36" fill="#E6ECF2"/>
  <rect x="708" y="306" width="462" height="36" fill="#8FA0B2"/>
  <text x="1512" y="332" text-anchor="end" font-family="JetBrainsMono Nerd Font" font-size="21" fill="var(--dark-foreground)">285.46 ms</text>

  <!-- Bar 3: deltalake cold 397.34 ms -> 644 px -->
  <text x="708" y="374" font-family="Noto Sans" font-size="18" fill="var(--dark-foreground)">deltalake 1.7.0 (cold CLI)</text>
  <rect x="708" y="386" width="648" height="36" fill="#E6ECF2"/>
  <rect x="708" y="386" width="644" height="36" fill="#B8C5D2"/>
  <text x="1512" y="412" text-anchor="end" font-family="JetBrainsMono Nerd Font" font-size="21" fill="var(--dark-foreground)">397.34 ms</text>

  <!-- Panel separator -->
  <line x1="708" y1="456" x2="1512" y2="456" stroke="#CBD5E0" stroke-width="1.5"/>

  <!-- Right column: Panel B - Peak RSS comparison (bar width 648px = 2100 MB => 0.3086 px/MB) -->
  <text x="708" y="500" font-family="Noto Sans" font-size="23" font-weight="700" fill="var(--foreground)">Peak memory (RSS) on the same 50,000-file replay</text>

  <!-- Bar 1: deltalens 89.4 MB -> 28 px -->
  <text x="708" y="546" font-family="Noto Sans" font-size="18" font-weight="700" fill="var(--foreground)">deltalens 2.0.1</text>
  <rect x="708" y="558" width="648" height="34" fill="#E6ECF2"/>
  <rect x="708" y="558" width="28" height="34" fill="var(--accent)"/>
  <text x="1512" y="583" text-anchor="end" font-family="JetBrainsMono Nerd Font" font-size="21" font-weight="700" fill="var(--accent)">89.4 MB</text>

  <!-- Bar 2: deltalake 2048.9 MB -> 632 px -->
  <text x="708" y="624" font-family="Noto Sans" font-size="18" fill="var(--dark-foreground)">deltalake 1.7.0 (Python 3.12)</text>
  <rect x="708" y="636" width="648" height="34" fill="#E6ECF2"/>
  <rect x="708" y="636" width="632" height="34" fill="#B8C5D2"/>
  <text x="1512" y="661" text-anchor="end" font-family="JetBrainsMono Nerd Font" font-size="21" fill="var(--dark-foreground)">2,048.9 MB</text>

  <!-- Bottom checkpointed summary box (two cleanly structured columns) -->
  <rect x="708" y="704" width="804" height="92" fill="#E4EBF2"/>
  <rect x="708" y="704" width="6" height="92" fill="var(--accent)"/>
  <text x="734" y="737" font-family="Noto Sans" font-size="16" font-weight="700" fill="var(--foreground)">60-commit checkpointed table (real Delta inspect)</text>
  <text x="734" y="772" font-family="JetBrainsMono Nerd Font" font-size="16" font-weight="700" fill="var(--accent)">deltalens: 2.91 ms / 12.5 MB</text>
  <line x1="1044" y1="752" x2="1044" y2="780" stroke="#A9B8C8" stroke-width="1.5"/>
  <text x="1064" y="772" font-family="JetBrainsMono Nerd Font" font-size="15" fill="var(--foreground)">deltalake: 56.50 ms (3.74 ms warm) / 160.9 MB</text>
</svg>
```
