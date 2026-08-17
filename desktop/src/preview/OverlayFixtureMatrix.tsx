import { overlayStageMode } from "@/lib/statusPresentation";
import { STATUS_FIXTURES } from "./statusFixtures";

/**
 * Development-only visual regression board.
 * Open `?preview=overlay-matrix`; each frame is the overlay's native width.
 */
export default function OverlayFixtureMatrix() {
  return (
    <main className="min-h-screen bg-[#111114] px-6 py-8 text-white">
      <div className="mx-auto max-w-[1280px]">
        <h1 className="text-xl font-semibold tracking-tight">
          Boris overlay fixture matrix
        </h1>
        <p className="mt-1 text-sm text-white/55">
          Deterministic browser-only states · presence 380 × 160 · thought 380 ×
          216 · card 400 × 300
        </p>

        <div className="mt-6 grid gap-5 [grid-template-columns:repeat(auto-fit,minmax(380px,1fr))]">
          {STATUS_FIXTURES.map((item) => {
            const mode = overlayStageMode(item.status);
            const frame =
              mode === "card"
                ? { label: "Open at 400 × 300", className: "h-[300px] w-[400px]" }
                : mode === "thought"
                  ? { label: "Open at 380 × 216", className: "h-[216px] w-[380px]" }
                  : { label: "Open at 380 × 120", className: "h-[160px] w-[380px]" };
            return (
            <section key={item.name} className="min-w-0">
              <div className="mb-2 flex items-baseline justify-between gap-3 px-1">
                <h2 className="text-xs font-medium text-white/80">
                  {item.label}
                </h2>
                <a
                  className="text-[11px] text-white/40 underline-offset-2 hover:text-white/70 hover:underline"
                  href={`/?window=overlay&fixture=${item.name}`}
                >
                  {frame.label}
                </a>
              </div>
              <iframe
                title={`${item.label} overlay fixture`}
                src={`/?window=overlay&fixture=${item.name}`}
                className={`block overflow-hidden rounded-lg border border-white/10 bg-black shadow-xl ${frame.className}`}
              />
            </section>
            );
          })}
        </div>
      </div>
    </main>
  );
}
