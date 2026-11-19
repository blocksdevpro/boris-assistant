import { ArrowRight } from "lucide-react";
import Link from "next/link";
import { pageMetadata, repo } from "@/lib/site";

export const metadata = pageMetadata({
  title: "Private voice assistant for Windows: what stays on your PC",
  description:
    "Boris runs wake word, speech-to-text, and voice output locally on Windows. See exactly what stays on your PC, what goes to your AI model, and how to turn memory off.",
  path: "/privacy",
});

const onDevice = [
  ["Wake word", "A local ONNX classifier listens for the wake word. Audio is scored in memory and never uploaded."],
  ["Speech-to-text", "NVIDIA Parakeet transcribes your request locally. Raw microphone audio stays on your PC."],
  ["Voice output", "Supertonic speech synthesis turns replies into audio locally."],
  ["Memory and sessions", "Facts, preferences, transcripts, and logs are stored under your Boris home folder in a local SQLite database and plain files."],
];

const leaves = [
  ["Your AI model", "The transcribed text of your request, plus relevant conversation, memory, and tool results, goes to the model you choose through OpenRouter. Their privacy policy applies to that request."],
  ["Web search", "When Boris searches the web, the query goes to Exa if you added an Exa key, or to DuckDuckGo otherwise. Pages it reads are fetched from their sites."],
  ["Downloads and updates", "Speech models download once from Hugging Face. Update checks contact GitHub Releases."],
];

export default function PrivacyPage() {
  return (
    <main id="main-content" className="content-page section-shell">
      <p className="story-label"><span /> Local voice on Windows</p>
      <h1>A private voice assistant for Windows</h1>
      <p className="page-intro">
        Boris listens, transcribes, and speaks on your own PC. Only the text of what you ask, and the context needed to answer it, goes to the AI model you pick. This page lists every place your data goes.
      </p>

      <section className="guide-section" aria-labelledby="local-title">
        <h2 id="local-title">What runs on your PC</h2>
        <dl className="fact-list">
          {onDevice.map(([term, detail]) => (
            <div key={term}><dt>{term}</dt><dd>{detail}</dd></div>
          ))}
        </dl>
        <p>
          After the speech models are installed, wake detection, transcription, and spoken replies keep working without an internet connection. Boris has no analytics or usage telemetry, and ONNX Runtime telemetry is switched off.
        </p>
      </section>

      <section className="guide-section" aria-labelledby="network-title">
        <h2 id="network-title">What leaves your PC</h2>
        <dl className="fact-list">
          {leaves.map(([term, detail]) => (
            <div key={term}><dt>{term}</dt><dd>{detail}</dd></div>
          ))}
        </dl>
        <p>
          You choose the model and the provider. OpenRouter lets you pin a provider, and model usage is billed to your own account.
        </p>
      </section>

      <section className="guide-section" aria-labelledby="control-title">
        <h2 id="control-title">Stay in control</h2>
        <ul className="plain-list">
          <li><strong>Turn memory off.</strong> Set <code>BORIS_MEMORY=0</code> before launching Boris to disable long-term memory.</li>
          <li><strong>Limit what Boris can do.</strong> Voice-safe, local-only, and full capability presets decide which tools exist at all.</li>
          <li><strong>Approve risky actions.</strong> Writes, shell commands, and other risky tool calls pause for your yes or no, by voice or on screen.</li>
          <li><strong>Delete everything.</strong> Your data lives in <code>%USERPROFILE%\.boris</code>. Remove that folder to erase memory, sessions, logs, and keys.</li>
        </ul>
        <p>
          API keys are stored in plain text in <code>auth.json</code> inside that folder, so protect your Windows account. Approval prompts and path rules reduce risk, but Boris is not a full sandbox. Read the <a href={`${repo}/blob/main/SECURITY.md`}>security policy</a> for details.
        </p>
      </section>

      <section className="guide-section" aria-labelledby="verify-title">
        <h2 id="verify-title">Check it yourself</h2>
        <p>
          Boris is open source under Apache 2.0, so every claim on this page can be verified in the code. The voice pipeline, agent tools, and network calls all live in one <a href={repo}>public repository</a>.
        </p>
        <Link href="/download" className="text-link">Download Boris for Windows <ArrowRight size={16} /></Link>
      </section>
    </main>
  );
}
