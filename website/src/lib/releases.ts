import { repo } from "@/lib/site";

// Verified against published GitHub releases, not the working-tree changelog.
export const stableRelease = {
  version: "1.1.0",
  date: "2026-08-15",
  dateLabel: "August 15, 2026",
  channel: "Stable",
  url: `${repo}/releases/tag/v1.1.0`,
  download: `${repo}/releases/download/v1.1.0/Boris_1.1.0_x64-setup.exe`,
  msi: `${repo}/releases/download/v1.1.0/Boris_1.1.0_x64_en-US.msi`,
  summary: "Faster voice replies, background research, and a desk for your results.",
  highlights: [
    "Speech starts sentence by sentence, with Silero voice activity detection and a responsive Stop action.",
    "Faster model routing and tool discovery, plus web search without a separate search API key.",
    "Research can run in background agents. Markdown and code results appear as cards in Home and the voice overlay.",
    "Optional Start with Windows launches Boris in the tray and starts the voice engine at sign-in.",
    "Choose Stable or Beta updates in Settings. Stable has both EXE and MSI installers for Windows x64.",
  ],
};

export const betaRelease = {
  version: "1.2.0-beta.2",
  date: "2026-08-28",
  dateLabel: "August 28, 2026",
  channel: "Beta",
  url: `${repo}/releases/tag/v1.2.0-beta.2`,
  download: `${repo}/releases/download/v1.2.0-beta.2/Boris_1.2.0-beta.2_x64-setup.exe`,
  summary: "Local, evidence-backed memory for facts, preferences, projects, and conversations.",
  highlights: [
    "A local SQLite store brings conversation evidence, durable facts, preferences, and projects into one memory system.",
    "Boris can search, retrieve, and forget evidence-backed memories. Completed conversations are saved to the store.",
    "Older profile, workspace, and session memories are imported and verified before the original files are retired.",
    "Includes the previous beta's taught wake filtering, echo cancellation, wake-word interruption, typed input, and on-screen reasoning preview.",
  ],
};
