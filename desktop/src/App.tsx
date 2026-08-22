import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { StatusPreviewProvider } from "@/bridge/useStatus";
import { OFF_STATUS, type StatusPicture } from "@/bridge";
import { AppErrorBoundary } from "@/components/AppErrorBoundary";
import { GridPresence } from "@/components/presence";
import { StartupScreen } from "@/components/StartupScreen";
import { logger } from "@/lib/logger";
import { isTauriRuntime } from "@/lib/runtime";
const MainWindow = lazy(() =>
  import("@/windows/main/MainWindow").then((m) => ({ default: m.MainWindow })),
);
const OverlayWindow = lazy(() =>
  import("@/windows/overlay/OverlayWindow").then((m) => ({
    default: m.OverlayWindow,
  })),
);

type Surface = "main" | "overlay";

function SurfaceFallback({
  label,
  transparent = false,
}: {
  label: string;
  transparent?: boolean;
}) {
  return (
    <div
      className="surface-loader"
      data-transparent={transparent || undefined}
      role="status"
      aria-live="polite"
      aria-label={label}
    >
      <GridPresence state="starting" reducedMotion={false} size="md" />
      <span className="surface-loader__label">{label}</span>
    </div>
  );
}

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

function isStartupPreview(): boolean {
  if (!import.meta.env.DEV || isTauriRuntime()) return false;
  return (
    new URLSearchParams(window.location.search).get("preview") === "startup"
  );
}

/**
 * One SPA, two surfaces.
 * Tauri loads the same frontend for every window; we pick the tree
 * from the window label (and a ?window= query for plain Vite).
 */
function resolveSurface(): Surface {
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
  const [surface] = useState<Surface>(resolveSurface);
  const fixtureMatrix = isOverlayFixtureMatrix();
  const startupPreview = isStartupPreview();
  const fixtureName = devFixtureName();
  const [fixtureStatus, setFixtureStatus] = useState<StatusPicture | null>(null);
  const [startupRevealed, setStartupRevealed] = useState(false);
  const [startupComplete, setStartupComplete] = useState(false);

  const revealStartup = useCallback(() => setStartupRevealed(true), []);
  const completeStartup = useCallback(() => setStartupComplete(true), []);

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
    if (!fixtureMatrix && !startupPreview) {
      logger.info("surface resolved", { surface });
    }
  }, [fixtureMatrix, startupPreview, surface]);

  // Overlay must stay in overlay-mode (transparent chrome); main keeps dark theme.
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
      // Overlay must keep this class: removing it lets startup.css paint
      // html/body as a solid slab (and remount in StrictMode would flash it).
      if (surface !== "overlay") {
        root.classList.remove("overlay-mode");
      }
    };
  }, [surface]);

  if (fixtureMatrix) {
    return (
      <AppErrorBoundary>
        <Suspense
          fallback={<SurfaceFallback label="Preparing preview gallery" />}
        >
          <OverlayFixtureMatrix />
        </Suspense>
      </AppErrorBoundary>
    );
  }

  if (startupPreview) {
    return (
      <AppErrorBoundary>
        <StartupScreen preview />
      </AppErrorBoundary>
    );
  }

  if (fixtureName && !fixtureStatus) {
    return <SurfaceFallback label="Preparing preview" />;
  }

  const surfaceView =
    surface === "overlay" ? (
      <Suspense
        fallback={
          <SurfaceFallback label="Waking Boris" transparent />
        }
      >
        <OverlayWindow />
      </Suspense>
    ) : (
      <Suspense fallback={<SurfaceFallback label="Opening Boris" />}>
        <MainWindow />
      </Suspense>
    );

  const surfaceContent = fixtureStatus ? (
    <StatusPreviewProvider status={fixtureStatus}>
      {surfaceView}
    </StatusPreviewProvider>
  ) : (
    surfaceView
  );

  const showStartup = surface === "main" && !fixtureName;

  return (
    <AppErrorBoundary>
      {showStartup ? (
        <>
          <div
            className="startup-app-shell"
            data-revealed={startupRevealed || startupComplete}
          >
            {surfaceContent}
          </div>
          {!startupComplete ? (
            <StartupScreen
              onReveal={revealStartup}
              onComplete={completeStartup}
            />
          ) : null}
        </>
      ) : (
        surfaceContent
      )}
    </AppErrorBoundary>
  );
}

export default App;
