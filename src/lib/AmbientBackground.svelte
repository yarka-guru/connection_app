<div class="ambient" aria-hidden="true">
  <span class="orb orb-1"></span>
  <span class="orb orb-2"></span>
  <span class="orb orb-3"></span>
  <span class="noise"></span>
</div>

<style>
  .ambient {
    position: fixed;
    inset: 0;
    overflow: hidden;
    pointer-events: none;
    z-index: 0;
  }

  .orb {
    position: absolute;
    border-radius: 50%;
    filter: blur(80px);
    opacity: 0.55;
    will-change: transform;
  }

  .orb-1 {
    width: 48vmax;
    height: 48vmax;
    top: -14vmax;
    left: -14vmax;
    background: radial-gradient(circle at 40% 40%, var(--bg-orb-1), transparent 70%);
    animation: ca-orb-drift-1 22s ease-in-out infinite;
  }

  .orb-2 {
    width: 56vmax;
    height: 56vmax;
    right: -18vmax;
    bottom: -20vmax;
    background: radial-gradient(circle at 60% 60%, var(--bg-orb-2), transparent 65%);
    opacity: 0.45;
    animation: ca-orb-drift-2 28s ease-in-out infinite;
  }

  .orb-3 {
    width: 40vmax;
    height: 40vmax;
    top: 30%;
    left: 40%;
    background: radial-gradient(circle at 50% 50%, var(--bg-orb-3), transparent 60%);
    opacity: 0.35;
    animation: ca-orb-drift-3 26s ease-in-out infinite;
  }

  .noise {
    position: absolute;
    inset: 0;
    background-image: var(--bg-noise);
    background-size: 200px 200px;
    mix-blend-mode: overlay;
    opacity: 0.8;
    pointer-events: none;
  }

  @media (prefers-reduced-transparency: reduce) {
    .ambient { display: none; }
  }

  @media (prefers-reduced-motion: reduce) {
    .orb { animation: none; }
  }
</style>
