import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { MainWindow } from "@/windows/main/MainWindow";
import { OverlayWindow } from "@/windows/overlay/OverlayWindow";

type Surface = "main" | "overlay";

/**
 * One SPA, two surfaces.
 * Tauri loads the same frontend for every window; we pick the tree
 * from the window label (and a ?window= query for plain Vite).
 */
async function resolveSurface(): Promise<Surface> {
  try {
    const label = getCurrentWindow().label;
    if (label === "overlay") return "overlay";
    if (label === "main") return "main";
  } catch {
    // Not under Tauri (browser / Vite only).
  }

  const params = new URLSearchParams(window.location.search);
  if (params.get("window") === "overlay") return "overlay";
  return "main";
}

function App() {
  const [surface, setSurface] = useState<Surface | null>(null);

  useEffect(() => {
    void resolveSurface().then(setSurface);
  }, []);

  // Overlay paints pure-black chrome (Windows color-key); main keeps dark theme.
  useEffect(() => {
    const root = document.documentElement;
    const meta = document.querySelector('meta[name="color-scheme"]');
    if (surface === "overlay") {
      root.classList.add("overlay-mode");
      root.classList.remove("dark");
      meta?.setAttribute("content", "only light");
    } else if (surface === "main") {
      root.classList.remove("overlay-mode");
      root.classList.add("dark");
      meta?.setAttribute("content", "dark");
    }
    return () => {
      root.classList.remove("overlay-mode");
    };
  }, [surface]);

  if (surface === null) {
    return (
      <div className="flex h-screen items-center justify-center bg-background text-sm text-muted-foreground">
        Loading…
      </div>
    );
  }

  if (surface === "overlay") {
    return <OverlayWindow />;
  }

  return <MainWindow />;
}

export default App;
