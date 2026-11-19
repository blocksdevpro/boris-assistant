import { ArrowRight } from "lucide-react";
import Link from "next/link";
import { pageMetadata } from "@/lib/site";
import { stableRelease } from "@/lib/releases";

export const metadata = pageMetadata({
  title: "Cortana alternative for Windows 10 and 11: a free voice assistant",
  description:
    "Looking for a Cortana replacement? Boris is a free, open-source voice assistant for Windows 10 and 11 with a wake word, local speech, web search, file tools, and memory.",
  path: "/cortana-alternative",
});

const tasks = [
  ["Ask a question out loud", "Say the wake word, ask, and hear the answer. Boris searches the web when it needs current information."],
  ["Work with your files", "Find, read, summarize, and organize files in folders you allow. Changes pause for your approval."],
  ["Remember what matters", "Boris can keep notes about your preferences and projects in a local memory you can search or clear."],
  ["Run desktop actions", "With the right capability preset, Boris can run commands on your PC, asking first when an action is risky."],
  ["Type when talking is awkward", "Paste long text, type exact values, or pick numbered choices in the app instead of speaking."],
];

export default function CortanaAlternativePage() {
  return (
    <main id="main-content" className="content-page section-shell">
      <p className="story-label"><span /> Voice assistant for Windows</p>
      <h1>A Cortana alternative for Windows 10 and 11</h1>
      <p className="page-intro">
        Microsoft retired the Cortana app on Windows in 2023. If you miss saying a wake word and getting help with your PC, Boris is a free, open-source voice assistant that runs as a Windows desktop app.
      </p>

      <section className="guide-section" aria-labelledby="tasks-title">
        <h2 id="tasks-title">What you can do with Boris</h2>
        <dl className="fact-list">
          {tasks.map(([term, detail]) => (
            <div key={term}><dt>{term}</dt><dd>{detail}</dd></div>
          ))}
        </dl>
      </section>

      <section className="guide-section" aria-labelledby="different-title">
        <h2 id="different-title">How Boris differs from Cortana</h2>
        <ul className="plain-list">
          <li><strong>Local speech.</strong> Wake detection, transcription, and spoken replies run on your PC. Only the text of your request goes to your AI model. <Link href="/privacy">See what stays on your PC</Link>.</li>
          <li><strong>Your choice of AI model.</strong> Boris connects to models through your own OpenRouter API key, so you pick the model and pay your provider directly.</li>
          <li><strong>Approval before risky actions.</strong> Capability presets decide which tools Boris has, and risky steps wait for your yes or no.</li>
          <li><strong>Open source.</strong> The whole app is Apache 2.0 licensed and built in Rust, so you can read it, change it, or add your own skills.</li>
        </ul>
      </section>

      <section className="guide-section" aria-labelledby="limits-title">
        <h2 id="limits-title">What Boris does not do</h2>
        <p>
          Boris is not built into Windows and does not use a Microsoft account. It does not manage Outlook calendars, reminders, or smart-home devices out of the box. It needs an OpenRouter API key for answers, and model usage is billed by your provider.
        </p>
      </section>

      <section className="guide-section" aria-labelledby="start-title">
        <h2 id="start-title">Get started</h2>
        <p>
          Boris {stableRelease.version} supports Windows 10 and 11 on x64 PCs with a microphone and speakers or headphones. Setup takes four steps: install the app, download the speech models, add your API key, and start a voice session.
        </p>
        <Link href="/download" className="text-link">Download Boris and follow the setup guide <ArrowRight size={16} /></Link>
      </section>
    </main>
  );
}
