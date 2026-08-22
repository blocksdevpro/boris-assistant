/** 3×3 LED matrices. 1 is lit. Shape language from SmoothUI Grid Loader. */

export type Cell = 0 | 1;
export type GridMatrix = [
  [Cell, Cell, Cell],
  [Cell, Cell, Cell],
  [Cell, Cell, Cell],
];

export type PresenceState =
  | "off"
  | "starting"
  | "ready"
  | "awaiting-reply"
  | "confirm"
  | "hearing"
  | "reading"
  | "thinking"
  | "working"
  | "talking"
  | "fault";

export type GridPlay = {
  frames: GridMatrix[];
  mode: "pulse" | "sequence" | "stagger" | "static";
  /** One cycle, milliseconds. */
  speed: number;
};

const G = {
  breathing: [
    [0, 0, 0],
    [0, 1, 0],
    [0, 0, 0],
  ],
  diamond: [
    [0, 1, 0],
    [1, 0, 1],
    [0, 1, 0],
  ],
  rippleIn: [
    [0, 0, 0],
    [0, 1, 0],
    [0, 0, 0],
  ],
  rippleOut: [
    [1, 1, 1],
    [1, 0, 1],
    [1, 1, 1],
  ],
  heartbeat: [
    [0, 1, 0],
    [1, 1, 1],
    [0, 1, 0],
  ],
  rain: [
    [0, 1, 0],
    [0, 1, 0],
    [0, 0, 0],
  ],
  waterfall: [
    [1, 1, 0],
    [0, 0, 0],
    [0, 0, 0],
  ],
  rainRev: [
    [0, 0, 0],
    [0, 1, 0],
    [0, 1, 0],
  ],
  snake: [
    [1, 1, 0],
    [0, 1, 0],
    [0, 0, 0],
  ],
  snake2: [
    [0, 1, 1],
    [0, 0, 1],
    [0, 0, 0],
  ],
  snake3: [
    [0, 0, 1],
    [0, 0, 1],
    [0, 1, 1],
  ],
  twinkle: [
    [1, 0, 0],
    [0, 0, 1],
    [0, 1, 0],
  ],
  sparkle: [
    [1, 0, 1],
    [0, 1, 0],
    [1, 0, 1],
  ],
  cross: [
    [0, 1, 0],
    [1, 1, 1],
    [0, 1, 0],
  ],
  xShape: [
    [1, 0, 1],
    [0, 1, 0],
    [1, 0, 1],
  ],
  spiral1: [
    [1, 1, 1],
    [0, 0, 1],
    [0, 0, 1],
  ],
  spiral2: [
    [0, 0, 1],
    [0, 0, 1],
    [1, 1, 1],
  ],
  spiral3: [
    [1, 0, 0],
    [1, 0, 0],
    [1, 1, 1],
  ],
  spiral4: [
    [1, 1, 1],
    [1, 0, 0],
    [1, 0, 0],
  ],
  edgeTop: [
    [1, 1, 1],
    [0, 0, 0],
    [0, 0, 0],
  ],
  edgeRight: [
    [0, 0, 1],
    [0, 0, 1],
    [0, 0, 1],
  ],
  edgeBot: [
    [0, 0, 0],
    [0, 0, 0],
    [1, 1, 1],
  ],
  edgeLeft: [
    [1, 0, 0],
    [1, 0, 0],
    [1, 0, 0],
  ],
  frame: [
    [1, 1, 1],
    [1, 0, 1],
    [1, 1, 1],
  ],
} as const satisfies Record<string, GridMatrix>;

export const PRESENCE_PLAY: Record<PresenceState, GridPlay> = {
  off: { frames: [G.breathing], mode: "static", speed: 0 },
  ready: { frames: [G.breathing], mode: "pulse", speed: 1680 },
  starting: {
    frames: [G.spiral1, G.spiral2, G.spiral3, G.spiral4],
    mode: "sequence",
    speed: 420,
  },
  hearing: {
    frames: [G.rain, G.waterfall, G.rainRev],
    mode: "sequence",
    speed: 380,
  },
  reading: {
    frames: [G.snake, G.snake2, G.snake3],
    mode: "sequence",
    speed: 360,
  },
  thinking: {
    frames: [G.rippleIn, G.diamond],
    mode: "sequence",
    speed: 1080,
  },
  working: {
    frames: [G.edgeTop, G.edgeRight, G.edgeBot, G.edgeLeft, G.frame],
    mode: "sequence",
    speed: 340,
  },
  talking: { frames: [G.heartbeat], mode: "pulse", speed: 620 },
  "awaiting-reply": { frames: [G.twinkle], mode: "pulse", speed: 1320 },
  confirm: { frames: [G.cross], mode: "pulse", speed: 900 },
  fault: { frames: [G.xShape], mode: "static", speed: 0 },
};

export const STATE_ACCENTS: Record<PresenceState, string> = {
  off: "#8e8e93",
  starting: "#0a84ff",
  ready: "#64d2ff",
  "awaiting-reply": "#0a84ff",
  confirm: "#ff9f0a",
  hearing: "#0a84ff",
  reading: "#7d7aff",
  thinking: "#7d7aff",
  working: "#7d7aff",
  talking: "#64d2ff",
  fault: "#ff453a",
};

export function playForState(state: PresenceState): GridPlay {
  return PRESENCE_PLAY[state];
}
