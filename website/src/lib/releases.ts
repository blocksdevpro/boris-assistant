import { repo } from "@/lib/site";

// Verified against published GitHub releases, not the working-tree changelog.
export const stableRelease = {
  version: "1.2.0",
  date: "2026-09-28",
  dateLabel: "September 28, 2026",
  channel: "Stable",
  url: `${repo}/releases/tag/v1.2.0`,
  download: `${repo}/releases/download/v1.2.0/Boris_1.2.0_x64-setup.exe`,
  msi: `${repo}/releases/download/v1.2.0/Boris_1.2.0_x64_en-US.msi`,
  summary: "Live voice feedback, stronger wake detection, and evidence-backed local memory.",
  highlights: [
    "Teach Boris your wake word with four samples. An optional speaker filter and audio cleanup improve wake detection and transcription.",
    "Interrupt Boris during speech, thinking, or confirmation. Voice prompts recover from device changes and unclear replies.",
    "A local SQLite store keeps evidence for facts, preferences, and projects, with memory search and explicit forgetting.",
    "Type exact values, secrets, large pastes, or numbered choices into Home or the overlay when speech is a poor fit.",
    "Follow voice turns through the presence orb, live transcription previews, tool progress, and streamed captions.",
    "Built-in playbooks cover coding, debugging, writing, review, and more. Stable offers EXE and MSI installers for Windows x64.",
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
