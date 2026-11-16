// Report the first painted frame to Rust (double rAF: the callback after the first paint).
requestAnimationFrame(() =>
  requestAnimationFrame(() => window.__TAURI__.core.invoke("first_frame")),
);
