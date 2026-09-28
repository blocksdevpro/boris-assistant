export type CaptionKind = "heard" | "said" | "error";

export type Caption = {
  kind: CaptionKind;
  text: string;
};

export type BargeInStage =
  | "listening"
  | "transcribing"
  | "switching"
  | "stopping";

export type OverlayPresence = {
  /** 1–2 word phase title */
  primary: string;
  /** Complementary detail — never a synonym of primary */
  secondary: string;
};

export type OverlayStageMode = "presence" | "thought" | "card";

export type ConversationLine =
  | { kind: "you"; text: string; muted?: boolean }
  | { kind: "boris"; text: string }
  | { kind: "status"; text: string }
  | { kind: "thought"; text: string }
  | { kind: "confirm"; activity: string | null; prompt: string }
  | { kind: "input"; label: string; prompt: string }
  | { kind: "error"; text: string }
  | { kind: "placeholder"; text: string };