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
  /** One animation cycle, in milliseconds. */
  speed: number;
  /**
   * Cell indexes in the order they should animate for a staggered pattern.
   * This lets an active state communicate direction instead of simply
   * following the DOM's row-major order.
   */
  staggerOrder?: readonly number[];
};

const G = {
  dot: [
    [0, 0, 0],
    [0, 1, 0],
    [0, 0, 0],
  ],
  cross: [
    [0, 1, 0],
    [1, 1, 1],
    [0, 1, 0],
  ],
  scanLine: [
    [0, 0, 0],
    [1, 1, 1],
    [0, 0, 0],
  ],
  equalizer: [
    [0, 1, 0],
    [1, 1, 1],
    [1, 1, 1],
  ],
  xShape: [
    [1, 0, 1],
    [0, 1, 0],
    [1, 0, 1],
  ],
  frame: [
    [1, 1, 1],
    [1, 0, 1],
    [1, 1, 1],
  ],
  ellipsis: [
    [0, 0, 0],
    [1, 1, 1],
    [0, 0, 0],
  ],
} as const satisfies Record<string, GridMatrix>;

const STAGGER_ORDER = {
  /** Outside edge, clockwise from the upper-left. */
  clockwiseFrame: [0, 1, 2, 5, 8, 7, 6, 3],
  /** A text-like scan across the middle row. */
  leftToRight: [3, 4, 5],
  /** A bottom-aligned three-bar voice meter. */
  equalizer: [6, 3, 7, 4, 1, 8, 5],
  /** A quiet three-dot prompt. */
  ellipsis: [3, 4, 5],
} as const;

export const PRESENCE_PLAY: Record<PresenceState, GridPlay> = {
  off: { frames: [G.dot], mode: "static", speed: 0 },
  ready: { frames: [G.dot], mode: "pulse", speed: 1_800 },
  starting: {
    frames: [G.frame],
    mode: "stagger",
    speed: 820,
    staggerOrder: STAGGER_ORDER.clockwiseFrame,
  },
  hearing: {
    frames: [G.equalizer],
    mode: "stagger",
    speed: 720,
    staggerOrder: STAGGER_ORDER.equalizer,
  },
  reading: {
    frames: [G.scanLine],
    mode: "stagger",
    speed: 760,
    staggerOrder: STAGGER_ORDER.leftToRight,
  },
  thinking: {
    frames: [G.frame],
    mode: "stagger",
    speed: 1_180,
    staggerOrder: STAGGER_ORDER.clockwiseFrame,
  },
  working: {
    frames: [G.frame],
    mode: "stagger",
    speed: 560,
    staggerOrder: STAGGER_ORDER.clockwiseFrame,
  },
  talking: {
    frames: [G.equalizer],
    mode: "stagger",
    speed: 500,
    staggerOrder: STAGGER_ORDER.equalizer,
  },
  "awaiting-reply": {
    frames: [G.ellipsis],
    mode: "stagger",
    speed: 1_460,
    staggerOrder: STAGGER_ORDER.ellipsis,
  },
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
