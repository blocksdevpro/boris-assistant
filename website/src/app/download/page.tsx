import { ArrowRight, Download, Monitor } from "lucide-react";
import Link from "next/link";
import { buttonVariants } from "@/components/ui/button";
import { betaRelease, stableRelease } from "@/lib/releases";
import { pageMetadata } from "@/lib/site";
import { cn } from "@/lib/utils";

export const metadata = pageMetadata({
  title: "Download Boris Assistant for Windows 10 and 11",
  description: `Download Boris Assistant for Windows x64. Choose stable ${stableRelease.version} or beta ${betaRelease.version}, compare installers, and set up local speech with your OpenRouter API key.`,
  path: "/download",
});

export default function DownloadPage() {
  return (
    <main id="main-content" className="content-page section-shell">
      <p className="story-label"><span /> Windows x64</p>
      <h1>Download Boris Assistant</h1>
      <p className="page-intro">A free, open-source voice assistant for Windows 10 and 11. Local speech models handle your voice, and your chosen AI model handles the request.</p>

      <section className="download-release" aria-labelledby="stable-title">
        <div>
          <span className="channel-label"><Monitor size={15} /> Recommended · Stable</span>
          <h2 id="stable-title">Boris {stableRelease.version}</h2>
          <p>Released <time dateTime={stableRelease.date}>{stableRelease.dateLabel}</time>. {stableRelease.summary}</p>
        </div>
        <div className="download-actions">
          <a href={stableRelease.download} className={cn(buttonVariants({ size: "lg" }), "primary-download")}><Download /> Download Windows EXE</a>
          <a href={stableRelease.msi} className="text-link">Windows MSI installer <ArrowRight size={16} /></a>
          <Link href="/releases#stable" className="text-link">Stable release notes <ArrowRight size={16} /></Link>
        </div>
      </section>

      <section className="guide-section" aria-labelledby="setup-title">
        <h2 id="setup-title">Set up Boris on Windows</h2>
        <ol className="setup-steps">
          <li><h3>Install the Windows app</h3><p>Download the EXE above and run the installer on a Windows 10 or 11 x64 PC. Stable also offers an MSI installer.</p></li>
          <li><h3>Install the local speech models</h3><p>Finish model installation on the first launch. Connect a microphone and speakers or headphones. Initial model downloads need an internet connection.</p></li>
          <li><h3>Connect your AI model</h3><p>Add your <a href="https://openrouter.ai/">OpenRouter</a> API key in Settings and choose a model. Boris is free. Model usage is billed by your provider.</p></li>
          <li><h3>Start a voice session</h3><p>Start the engine and say Boris, then ask a question. Capability presets control tool access, and risky actions ask for approval.</p></li>
        </ol>
      </section>

      <section id="beta" className="download-release" aria-labelledby="beta-title">
        <div>
          <span className="channel-label beta-label">Latest published beta</span>
          <h2 id="beta-title">Boris {betaRelease.version}</h2>
          <p>Released <time dateTime={betaRelease.date}>{betaRelease.dateLabel}</time>. {betaRelease.summary} This is a pre-release and uses an EXE installer only.</p>
          <p>Upgrading from beta.1 or earlier can migrate older memory using your configured LLM. Read the <Link href="/releases#beta">migration and privacy notes</Link> before upgrading.</p>
        </div>
        <div className="download-actions">
          <a href={betaRelease.download} className={cn(buttonVariants({ size: "lg" }), "beta-download")}><Download /> Download beta EXE</a>
          <a href={betaRelease.url} className="text-link">View beta on GitHub <ArrowRight size={16} /></a>
        </div>
      </section>

      <section className="guide-section">
        <h2>Keep Boris up to date</h2>
        <p>Choose your update channel in Settings → General → Updates. Stable follows stable releases. Beta follows published beta versions. Installing Boris does not require you to build it from source.</p>
        <Link href="/#faq" className="text-link">More about offline use, privacy, and tools <ArrowRight size={16} /></Link>
      </section>
    </main>
  );
}
