import {
  ArrowRight,
  AudioLines,
  Brain,
  Check,
  Code2,
  Download,
  FileText,
  Globe2,
  HardDrive,
  MemoryStick,
  Mic,
  Monitor,
  Search,
  Settings2,
  ShieldCheck,
  Terminal,
  Volume2,
} from "lucide-react";
import Image from "next/image";
import { buttonVariants } from "@/components/ui/button";
import { MarketingHeader } from "@/components/marketing-header";
import { HowItWorks } from "@/components/how-it-works";
import { cn } from "@/lib/utils";

const repo = "https://github.com/blocksdevpro/boris-assistant";
const releases = `${repo}/releases`;
const windowsInstaller = "/download";

function BorisMark({ className }: { className?: string }) {
  return (
    <Image
      src="/boris-mark.svg"
      alt=""
      width={24}
      height={24}
      className={cn("boris-mark", className)}
      aria-hidden="true"
    />
  );
}

function FloatingTile({
  className,
  icon: Icon,
}: {
  className: string;
  icon: React.ComponentType<{ className?: string; strokeWidth?: number }>;
}) {
  return (
    <div className={cn("floating-tile", className)} aria-hidden="true">
      <Icon strokeWidth={1.65} />
    </div>
  );
}

export default function Home() {
  return (
    <main className="min-h-screen overflow-x-clip bg-[#090a0c] text-[#f4f4f5] selection:bg-[#d9ff75] selection:text-[#11130c]">
      <MarketingHeader />

      <section id="top" className="hero relative min-h-[1080px] px-5 pt-[138px] sm:pt-[148px]">
        <div className="hero-grid" aria-hidden="true" />
        <FloatingTile className="tile-mic" icon={Mic} />
        <FloatingTile className="tile-brain" icon={Brain} />
        <FloatingTile className="tile-memory" icon={MemoryStick} />
        <FloatingTile className="tile-shield" icon={ShieldCheck} />
        <FloatingTile className="tile-search" icon={Search} />

        <div className="relative z-10 mx-auto flex max-w-4xl flex-col items-center text-center">
          <a href={`${releases}/tag/v1.1.0`} target="_blank" rel="noreferrer" className="launch-note hero-enter" style={{ "--delay": "0ms" } as React.CSSProperties}>
            <span /> Boris 1.1 is out for Windows <ArrowRight className="size-3.5" />
          </a>

          <h1 className="hero-title hero-enter" style={{ "--delay": "80ms" } as React.CSSProperties}>
            A voice assistant
            <br />
            <span>that can actually help.</span>
          </h1>

          <p className="hero-copy hero-enter" style={{ "--delay": "150ms" } as React.CSSProperties}>
            Boris listens, remembers, and gets things done on your computer. Local voice, your choice of model, and tools that ask before they act.
          </p>

          <div className="hero-actions hero-enter" style={{ "--delay": "220ms" } as React.CSSProperties}>
            <a href={windowsInstaller} download="Boris_1.1.0_x64-setup.exe" className={cn(buttonVariants({ size: "lg" }), "primary-download")}>
              <Monitor data-icon="inline-start" /> Download for Windows
            </a>
            <a href={repo} target="_blank" rel="noreferrer" className="source-link">
              <Code2 className="size-4" /> Browse the source <ArrowRight className="size-3.5 opacity-45" />
            </a>
            <p>Free and open source · Windows 10 and 11</p>
          </div>
        </div>

        <div className="hero-product hero-enter" style={{ "--delay": "290ms" } as React.CSSProperties}>
          <div className="product-shot-frame">
            <Image
              src="/boris-screenshot.png"
              alt="Boris desktop app showing a ready voice session and its current conversation"
              width={958}
              height={718}
              priority
              sizes="(max-width: 620px) calc(100vw - 16px), (max-width: 1120px) calc(100vw - 40px), 1050px"
              className="product-shot-image"
            />
          </div>
        </div>
      </section>

      <section className="proof-strip">
        <div className="mx-auto grid max-w-[1160px] sm:grid-cols-3">
          <div><strong>Local voice</strong><span>Wake word, speech-to-text, and voice output run on your PC.</span></div>
          <div><strong>Useful tools</strong><span>Files, web, memory, skills, and approved system actions.</span></div>
          <div><strong>Open source</strong><span>Apache 2.0 licensed, Rust-powered, and ready to fork.</span></div>
        </div>
      </section>

      <section id="how-it-works" className="section-shell py-28 sm:py-36">
        <div className="section-heading">
          <p><span /> One quiet loop</p>
          <h2>Ask naturally.<br />Boris handles the steps.</h2>
          <span className="section-summary">The whole interaction stays visible without becoming another app you have to manage.</span>
        </div>

        <HowItWorks />
      </section>

      <section id="features" className="section-shell pb-28 sm:pb-40">
        <div className="story-block story-voice">
          <div className="story-copy">
            <p className="story-label"><span /> Always within earshot</p>
            <h2>It shows up when you call. Then gets out of your way.</h2>
            <p>Boris lives in a small, always-on-top voice island—not a dashboard. You see what it heard, what it is doing, and when it needs your attention.</p>
            <ul>
              <li><Check /> Wake-word activation</li>
              <li><Check /> Live private captions</li>
              <li><Check /> Listening, thinking, and speaking states</li>
            </ul>
          </div>
          <div className="island-scene" aria-hidden="true">
            <span className="scene-caption scene-you"><small>You</small>Clean up my downloads folder.</span>
            <div className="large-island">
              <span className="large-orb"><i /></span>
              <span><strong>Listening</strong><small>Waiting for you to finish…</small></span>
              <span className="scene-wave">{[8,16,10,22,14,19,7].map((h, i) => <i key={i} style={{ height: h }} />)}</span>
            </div>
            <span className="scene-caption scene-boris"><small>Boris</small>I’ll sort by file type and leave recent files alone.</span>
          </div>
        </div>

        <div className="story-block story-tools">
          <div className="tool-scene" aria-hidden="true">
            <div className="tool-scene-head"><span className="tool-status" /> Working on it <small>3 steps</small></div>
            <div className="tool-row"><span><FileText /> Read Downloads</span><Check /></div>
            <div className="tool-row"><span><Search /> Group 42 files</span><Check /></div>
            <div className="tool-row tool-row-active"><span><Terminal /> Move files into folders</span><small>Needs approval</small></div>
            <div className="tool-confirm">
              <span><ShieldCheck /> Review before Boris continues</span>
              <div><span className="mock-action">Cancel</span><span className="mock-action mock-action-primary">Allow once</span></div>
            </div>
          </div>
          <div className="story-copy">
            <p className="story-label"><span /> Tools with judgment</p>
            <h2>Helpful enough to act. Careful enough to ask.</h2>
            <p>Boris can search, work with files, remember notes, and run system actions. Capability presets and human approval keep you in control.</p>
            <ul>
              <li><Check /> Voice-safe, local-only, and full presets</li>
              <li><Check /> Explicit approval for risky actions</li>
              <li><Check /> Sandboxed workspace and path controls</li>
            </ul>
          </div>
        </div>
      </section>

      <section className="local-section">
        <div className="section-shell local-layout py-24 sm:py-28">
          <div>
            <p className="story-label"><span /> Local where it matters</p>
            <h2 className="local-title">Your voice shouldn’t need a round trip to the cloud.</h2>
            <p className="local-copy">Wake detection, speech recognition, and speech generation run locally. Boris only sends the assistant request to the model provider you choose.</p>
          </div>
          <div className="local-stack">
            <div><span className="stack-icon"><AudioLines /></span><span><strong>Parakeet speech-to-text</strong><small>Runs locally with ONNX</small></span><em>On device</em></div>
            <div><span className="stack-icon"><Volume2 /></span><span><strong>Supertone voice</strong><small>Fast local speech output</small></span><em>On device</em></div>
            <div><span className="stack-icon"><HardDrive /></span><span><strong>Memory and sessions</strong><small>Stored in your Boris home</small></span><em>On device</em></div>
            <div className="provider-row"><span className="stack-icon"><Globe2 /></span><span><strong>Your chosen model</strong><small>Connected through OpenRouter</small></span><em className="cloud-label">Your provider</em></div>
          </div>
        </div>
      </section>

      <section id="open-source" className="section-shell py-28 sm:py-40">
        <div className="open-panel">
          <div>
            <p className="story-label"><span /> Yours to inspect</p>
            <h2>Open source from wake word to tool call.</h2>
            <p>Boris is built in Rust with a Tauri desktop shell. Read every layer, add your own skill, change the model, or ship your own version.</p>
            <div className="open-actions">
              <a href={repo} target="_blank" rel="noreferrer" className={cn(buttonVariants({ size: "lg" }), "open-primary")}><Code2 /> Explore the repository</a>
              <a href={`${repo}/blob/main/CONTRIBUTING.md`} target="_blank" rel="noreferrer">Contributing guide <ArrowRight /></a>
            </div>
          </div>
          <div className="repo-map" aria-label="Boris repository architecture">
            <p className="repo-caption">Four layers, one inspectable pipeline.</p>
            <div className="repo-head"><BorisMark className="h-4 w-5" /><strong>boris-assistant</strong><span>Apache 2.0</span></div>
            <div className="repo-row"><span><Mic /> Voice pipeline</span><small>Wake · VAD · STT · TTS</small></div>
            <div className="repo-row"><span><Brain /> Agent runtime</span><small>Tools · memory · sessions</small></div>
            <div className="repo-row"><span><Monitor /> Desktop</span><small>React · Tauri · Rust</small></div>
            <div className="repo-row"><span><Settings2 /> Capability policy</span><small>Presets · approvals · sandbox</small></div>
          </div>
        </div>
      </section>

      <section className="final-cta">
        <div className="final-ring" aria-hidden="true" />
        <div className="relative z-10 mx-auto flex max-w-3xl flex-col items-center px-5 text-center">
          <span className="final-mark"><BorisMark className="h-8 w-10" /></span>
          <h2>Talk to your computer.<br />Not around it.</h2>
          <p>Free, open source, and ready for Windows.</p>
          <span className="release-meta">Version 1.1.0 · Windows 10 and 11</span>
          <a href={windowsInstaller} download="Boris_1.1.0_x64-setup.exe" className={cn(buttonVariants({ size: "lg" }), "primary-download mt-8")}><Download /> Download Boris</a>
        </div>
      </section>

      <footer>
        <div className="mx-auto flex max-w-[1160px] flex-col items-center justify-between gap-5 px-5 py-7 sm:flex-row">
          <div className="footer-brand"><BorisMark className="h-4 w-5" /><span>Boris</span><small>© 2026 BlocksDevPro</small></div>
          <div className="footer-links"><a href={repo}>GitHub</a><a href={releases}>Release notes</a><a href={`${repo}/blob/main/SECURITY.md`}>Security</a><a href={`${repo}/blob/main/LICENSE`}>License</a></div>
        </div>
      </footer>
    </main>
  );
}
