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
  version: "1.3.0-beta.2",
  date: "2026-11-19",
  dateLabel: "November 19, 2026",
  channel: "Beta",
  url: `${repo}/releases/tag/v1.3.0-beta.2`,
  download: `${repo}/releases/download/v1.3.0-beta.2/Boris_1.3.0-beta.2_x64-setup.exe`,
  summary: "A guided first run, an on-device setup check, and clear answers when \"Hey Boris\" does nothing.",
  highlights: [
    "A welcome flow connects your OpenRouter key, picks fast replies or a lighter PC, chooses a microphone, and downloads the voice models. New installs start listening as soon as it finishes.",
    "A setup check runs \"Hey Boris, what time is it?\" through the real wake, speech-detection, and transcription models, then checks your mic and speakers. Repair re-downloads damaged models.",
    "Boris says when the mic hears only silence (Windows mic privacy, mute key, dead input) or when a wake was ignored by the speaker filter, even with every window hidden.",
    "Unplugging or switching a microphone or speaker no longer needs a restart. A soft chime confirms the moment \"Hey Boris\" is heard.",
    "The key check says whether your OpenRouter key works, is out of credit, or was rejected. Tray Quit no longer freezes, and a broken speech runtime is explained instead of hanging at \"Starting\".",
    "Includes everything in 1.3.0-beta.1: task-aware tool selection, parallel tool calls, and the developer monitor. EXE installer only; stable installations stay on 1.2.0 unless you choose the Beta channel.",
  ],
};
