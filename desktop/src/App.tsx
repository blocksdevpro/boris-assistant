import { lazy, Suspense, useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { StatusPreviewProvider } from "@/bridge/useStatus";
import { OFF_STATUS, type StatusPicture } from "@/bridge";
import { AppErrorBoundary } from "@/components/AppErrorBoundary";
import { logger } from "@/lib/logger";
import { isTauriRuntime } from "@/lib/runtime";
import { MainWindow } from "@/windows/main/MainWindow";
import { OverlayWindow } from "@/windows/overlay/OverlayWindow";

type Surface = "main" | "overlay";

const OverlayFixtureMatrix = lazy(
  () => import("@/preview/OverlayFixtureMatrix"),
);

function isOverlayFixtureMatrix(): boolean {
  if (!import.meta.env.DEV || isTauriRuntime()) return false;
  return (
    new URLSearchParams(window.location.search).get("preview") ===
    "overlay-matrix"
  );
}

function devFixtureName(): string | null {
  if (!import.meta.env.DEV || isTauriRuntime()) return null;
  return new URLSearchParams(window.location.search).get("fixture");
}

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
  const fixtureMatrix = isOverlayFixtureMatrix();
  const fixtureName = devFixtureName();
  const [fixtureStatus, setFixtureStatus] = useState<StatusPicture | null>(null);

  useEffect(() => {
    let active = true;
    if (!fixtureName) {
      setFixtureStatus(null);
      return;
    }
    void import("@/preview/statusFixtures").then(({ getStatusFixture }) => {
      if (!active) return;
      const status = getStatusFixture(fixtureName);
      if (!status) logger.warn("unknown browser preview fixture", { fixtureName });
      setFixtureStatus(status ?? OFF_STATUS);
    });
    return () => {
      active = false;
    };
  }, [fixtureName]);

  useEffect(() => {
    if (fixtureMatrix) return;
    void resolveSurface().then((s) => {
      logger.info("surface resolved", { surface: s });
      setSurface(s);
    });
  }, [fixtureMatrix]);

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

  if (fixtureMatrix) {
    return (
      <AppErrorBoundary>
        <Suspense
          fallback={
            <div className="flex h-screen items-center justify-center bg-[#111114] text-sm text-white/50">
              Loading fixture matrix…
            </div>
          }
        >
          <OverlayFixtureMatrix />
        </Suspense>
      </AppErrorBoundary>
    );
  }

  if (surface === null) {
    return (
      <div className="flex h-screen items-center justify-center bg-background text-sm text-muted-foreground">
        Loading…
      </div>
    );
  }

  if (fixtureName && !fixtureStatus) {
    return (
      <div className="flex h-screen items-center justify-center bg-background text-sm text-muted-foreground">
        Loading fixture…
      </div>
    );
  }

  const surfaceView =
    surface === "overlay" ? <OverlayWindow /> : <MainWindow />;

  return (
    <AppErrorBoundary>
      {fixtureStatus ? (
        <StatusPreviewProvider status={fixtureStatus}>
          {surfaceView}
        </StatusPreviewProvider>
      ) : (
        surfaceView
      )}
    </AppErrorBoundary>
  );
}

export default App;
