import { overlayStageMode } from "@/lib/statusPresentation";
import { STATUS_FIXTURES } from "./statusFixtures";

/**
 * Development-only visual regression board.
 * Open `?preview=overlay-matrix`; each frame is the overlay's native width.
 */
export default function OverlayFixtureMatrix() {
  const scaleParam = new URLSearchParams(window.location.search).get("scale");
  const requestedScale = scaleParam == null ? 100 : Number(scaleParam);
  const scale = Number.isFinite(requestedScale)
    ? Math.min(125, Math.max(75, requestedScale))
    : 100;

  return (
    <main className="min-h-screen bg-[#09090b] px-6 py-10 text-white selection:bg-sky-300/25">
      <div className="pointer-events-none fixed inset-0 bg-[radial-gradient(circle_at_50%_-15%,rgba(96,165,250,0.10),transparent_38%)]" />
      <div className="relative mx-auto max-w-[1280px]">
        <header className="mb-8 flex flex-wrap items-end justify-between gap-5">
          <div>
            <div className="mb-3 inline-flex items-center gap-2 rounded-full border border-white/[0.08] bg-white/[0.045] px-3 py-1 text-[11px] font-medium tracking-wide text-white/55 shadow-[inset_0_1px_0_rgba(255,255,255,0.05)] backdrop-blur-xl">
              <span className="size-1.5 rounded-full bg-emerald-400 shadow-[0_0_10px_rgba(52,211,153,0.6)]" />
              Live component gallery
            </div>
            <h1 className="text-[28px] font-semibold tracking-[-0.035em] text-white/95">
              Overlay states
            </h1>
            <p className="mt-1.5 max-w-xl text-[13px] leading-relaxed text-white/45">
              Deterministic states in the card-budget window the island morphs
              inside. Ready for motion, contrast, and clipping review.
            </p>
          </div>
          <div className="rounded-xl border border-white/[0.07] bg-white/[0.035] px-3 py-2 text-right text-[11px] leading-relaxed text-white/40 shadow-[inset_0_1px_0_rgba(255,255,255,0.04)] backdrop-blur-xl">
            <p>Native HWND + 32px shadow gutter</p>
            <div className="mt-1 flex items-center justify-end gap-1">
              {[75, 100, 125].map((value) => (
                <a
                  key={value}
                  href={`/?preview=overlay-matrix&scale=${value}`}
                  className={
                    value === scale
                      ? "rounded-md bg-white/[0.1] px-1.5 py-0.5 text-white/80"
                      : "rounded-md px-1.5 py-0.5 hover:bg-white/[0.06] hover:text-white/65"
                  }
                >
                  {value}%
                </a>
              ))}
            </div>
          </div>
        </header>

        <div className="grid gap-5 [grid-template-columns:repeat(auto-fit,minmax(380px,1fr))]">
          {STATUS_FIXTURES.map((item) => {
            const mode = overlayStageMode(item.status);
            const frame = {
              label: `${mode} · 464 × 364`,
              width: 464,
              height: 364,
            };
            const scaledWidth = frame.width * (scale / 100);
            const scaledHeight = frame.height * (scale / 100);

            return (
              <section
                key={item.name}
                className="group min-w-0 rounded-[18px] border border-white/[0.07] bg-white/[0.025] p-2.5 shadow-[0_18px_60px_rgba(0,0,0,0.2),inset_0_1px_0_rgba(255,255,255,0.04)] transition-[border-color,background-color,transform] duration-300 ease-out hover:-translate-y-0.5 hover:border-white/[0.11] hover:bg-white/[0.035]"
              >
                <div className="mb-2.5 flex items-baseline justify-between gap-3 px-1.5 pt-0.5">
                  <h2 className="text-[12px] font-medium tracking-[-0.01em] text-white/75">
                    {item.label}
                  </h2>
                  <a
                    className="rounded-md px-1.5 py-1 text-[10px] text-white/35 transition-colors hover:bg-white/[0.06] hover:text-white/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-300/40"
                    href={`/?window=overlay&fixture=${item.name}&scale=${scale}`}
                  >
                    {frame.label}
                  </a>
                </div>
                <iframe
                  title={`${item.label} overlay fixture`}
                  src={`/?window=overlay&fixture=${item.name}&scale=${scale}`}
                  loading="lazy"
                  style={{ width: scaledWidth, height: scaledHeight }}
                  className="block max-w-full overflow-hidden rounded-[13px] border border-white/[0.08] bg-black shadow-[0_12px_36px_rgba(0,0,0,0.38)]"
                />
              </section>
            );
          })}
        </div>
      </div>
    </main>
  );
}
