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
  version: "1.3.0-beta.1",
  date: "2026-10-08",
  dateLabel: "October 8, 2026",
  channel: "Beta",
  url: `${repo}/releases/tag/v1.3.0-beta.1`,
  download: `${repo}/releases/download/v1.3.0-beta.1/Boris_1.3.0-beta.1_x64-setup.exe`,
  summary: "Faster first replies through task-aware tool selection, explicit parallel tool calls, and a developer monitor.",
  highlights: [
    "Boris picks the tools a task needs before the first model request. Research gets web and recall tools; workspace work gets file and shell tools. Follow-ups such as \"continue\" keep the same set.",
    "Independent tool calls can run in parallel, with the batch shown in the activity chip. Writes, confirmations, and typed input keep their order.",
    "A developer monitor in the desktop app shows model requests, tool calls, routing, context, and provider usage, with export for debugging.",
    "Tool results keep their success or failure status separately from their text, so reading a log that mentions \"error\" no longer fails a successful call.",
    "Shell exits, missing paths, and MCP errors report failure with useful diagnostics. Failed report cards keep their full content on screen.",
    "Upgrades from 1.2.0 keep the existing local memory store. EXE installer only; stable installations stay on 1.2.0 unless you choose the Beta channel.",
  ],
};
