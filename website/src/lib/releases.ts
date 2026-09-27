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
  version: "1.2.0-beta.3",
  date: "2026-09-27",
  dateLabel: "September 27, 2026",
  channel: "Beta",
  url: `${repo}/releases/tag/v1.2.0-beta.3`,
  download: `${repo}/releases/download/v1.2.0-beta.3/Boris_1.2.0-beta.3_x64-setup.exe`,
  summary: "Live transcription, visible tool progress, and more reliable voice confirmations.",
  highlights: [
    "A presence orb shows listening, thinking, tool work, speaking, and input states. Reduced-motion settings keep it static.",
    "Live transcription previews appear during voice capture on supported speech models. Tool progress and streamed captions stay visible in a more stable overlay.",
    "Wake-word interruption works during confirmation prompts. Device changes restart the prompt, and a bare 'please' no longer approves an action.",
    "Typed input supports numbered choices, file tools suggest nearby names, and long tool results can be reread without repeating the operation.",
    "More reliable context compaction and streaming, clearer separation of trusted instructions from retrieved data, and bundled task playbooks.",
    "Retains beta.2's evidence-backed local memory, with search, retrieval, forgetting, and verified migration from older versions.",
  ],
};
